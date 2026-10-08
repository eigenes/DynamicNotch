//! Voice dictation: speak, and the text lands in the app you are typing in.
//!
//! Flow: hotkey (Ctrl+Alt+D) → record the default microphone (WASAPI,
//! 16 kHz mono) with a live level meter in the notch → Groq Whisper
//! transcription → optional LLM clean-up → paste into the window that was
//! focused when you started (the text also stays on the clipboard).
//!
//! The hotkey works two ways: tap it to start and tap again to stop, or hold
//! it while talking and let go to finish. The API key comes from GROQ_API_KEY
//! (environment or a git-ignored `.env` file), never from config.toml.

use std::any::Any;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP,
    VIRTUAL_KEY, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, IsWindow, SetForegroundWindow};

use super::{Activity, Cx, Module, ModuleId, Peek, Slot, SystemEvent};
use crate::bus::Bus;
use crate::config::VoiceCfg;
use crate::gfx::{Icon, TextStyle};
use crate::hotkeys;
use crate::sys::{audio, clip, dotenv, stt};
use crate::ui::{ButtonStyle, Ui};
use crate::util::{one_line, palette, truncate_chars, Color, Rect, SendHwnd};

const ID: ModuleId = "voice";
/// Holding the hotkey at least this long makes its release end the recording.
const HOLD: Duration = Duration::from_millis(350);
/// Shorter recordings are treated as accidental presses.
const MIN_SECONDS: f32 = 0.35;
/// Quieter recordings (RMS) contain no speech worth sending.
const MIN_RMS: f32 = 0.004;
const LEVELS: usize = 64;
const HISTORY: usize = 20;
const ROW_H: f32 = 38.0;
const MAX_ROWS: usize = 5;
const REC_COLOR: Color = palette::RED;

enum VoiceMsg {
    Level(u64, f32),
    Recorded(u64, Vec<i16>),
    Transcribed(u64, String),
    /// (generation, title, details)
    Failed(u64, &'static str, String),
}

#[derive(Clone, Copy, PartialEq)]
enum Phase {
    Idle,
    Recording,
    Transcribing,
}

struct Entry {
    text: String,
    at: Instant,
}

pub struct Voice {
    phase: Phase,
    /// Bumped per recording; late messages from an older one are ignored.
    gen: u64,
    stop: Option<Arc<AtomicBool>>,
    started_at: Instant,
    phase_at: Instant,
    /// When the hotkey press that started the recording happened.
    pressed_at: Option<Instant>,
    /// Window focused when recording started (where the text goes).
    target: isize,
    levels: VecDeque<f32>,
    history: Vec<Entry>,
    copied: Option<(usize, Instant)>,
    /// Last failure: (title, details).
    error: Option<(&'static str, String)>,
    dictate_vk: u16,
    hotkey: String,
    cfg: VoiceCfg,
    hwnd: SendHwnd,
    bus: Option<Bus>,
}

impl Voice {
    pub fn new() -> Self {
        Self {
            phase: Phase::Idle,
            gen: 0,
            stop: None,
            started_at: Instant::now(),
            phase_at: Instant::now(),
            pressed_at: None,
            target: 0,
            levels: VecDeque::with_capacity(LEVELS),
            history: Vec::new(),
            copied: None,
            error: None,
            dictate_vk: 0,
            hotkey: String::new(),
            cfg: VoiceCfg::default(),
            hwnd: SendHwnd(0),
            bus: None,
        }
    }

    fn apply_cfg(&mut self, cx: &Cx) {
        self.cfg = cx.cfg.voice.clone();
        self.hotkey = cx.cfg.hotkeys.dictate.clone();
        self.dictate_vk = hotkeys::parse(&self.hotkey).map_or(0, |(_, vk)| vk as u16);
    }

    fn set_phase(&mut self, p: Phase) {
        self.phase = p;
        self.phase_at = Instant::now();
    }

    fn start(&mut self, cx: &mut Cx, from_hotkey: bool) {
        if self.phase != Phase::Idle {
            return;
        }
        if dotenv::get(stt::KEY_VAR).is_none() {
            cx.fx.peek(
                Peek::new(
                    Icon::Mic,
                    palette::ORANGE,
                    "Dictation needs a Groq API key",
                    format!("Add {}=… to .env", stt::KEY_VAR),
                )
                .key("voice")
                .duration_ms(5000)
                .page(ID),
            );
            return;
        }
        let Some(bus) = self.bus.clone() else { return };
        let fg = unsafe { GetForegroundWindow() };
        self.target = if fg == self.hwnd.hwnd() { 0 } else { fg.0 as isize };
        self.gen += 1;
        self.error = None;
        self.levels.clear();
        self.pressed_at = from_hotkey.then_some(cx.now);
        self.started_at = cx.now;
        self.set_phase(Phase::Recording);
        let stop = Arc::new(AtomicBool::new(false));
        self.stop = Some(stop.clone());
        let (gen, max) = (self.gen, self.cfg.max_seconds);
        let _ = std::thread::Builder::new().name("voice-record".into()).spawn(move || {
            use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};
            unsafe {
                let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            }
            let r = audio::capture(&stop, max, |l| bus.to_module(ID, VoiceMsg::Level(gen, l)));
            unsafe { CoUninitialize() };
            match r {
                Ok(samples) => bus.to_module(ID, VoiceMsg::Recorded(gen, samples)),
                Err(e) => bus.to_module(ID, VoiceMsg::Failed(gen, "Couldn't record", e)),
            }
        });
        crate::log!("voice: recording");
    }

    /// Finish recording; the audio is transcribed once the recorder hands it over.
    fn finish(&mut self) {
        if self.phase != Phase::Recording {
            return;
        }
        if let Some(s) = self.stop.take() {
            s.store(true, Ordering::Relaxed);
        }
        self.pressed_at = None;
        self.set_phase(Phase::Transcribing);
    }

    fn cancel(&mut self) {
        if let Some(s) = self.stop.take() {
            s.store(true, Ordering::Relaxed);
        }
        self.gen += 1; // drop whatever is still in flight
        self.pressed_at = None;
        self.set_phase(Phase::Idle);
    }

    fn transcribe(&mut self, samples: Vec<i16>) {
        let (Some(bus), Some(key)) = (self.bus.clone(), dotenv::get(stt::KEY_VAR)) else { return };
        let cfg = self.cfg.clone();
        let gen = self.gen;
        self.set_phase(Phase::Transcribing);
        let _ = std::thread::Builder::new().name("voice-stt".into()).spawn(move || {
            let cancel = AtomicBool::new(false);
            let t0 = Instant::now();
            let wav = audio::wav(&samples);
            match stt::transcribe(&cfg, &key, &wav, &cancel) {
                Ok(t) => {
                    let t1 = Instant::now();
                    let text = stt::cleanup(&cfg, &key, &t.text, &t.language, &cancel);
                    crate::log!(
                        "voice: {:.1}s audio -> {} chars (stt {} ms, cleanup {} ms)",
                        samples.len() as f32 / audio::SAMPLE_RATE as f32,
                        text.chars().count(),
                        (t1 - t0).as_millis(),
                        t1.elapsed().as_millis()
                    );
                    bus.to_module(ID, VoiceMsg::Transcribed(gen, text));
                }
                Err(e) => bus.to_module(ID, VoiceMsg::Failed(gen, "Couldn't transcribe", e)),
            }
        });
    }

    /// Put `text` on the clipboard and paste it into `target` (or whatever is
    /// focused). The clipboard write happens here on the UI thread; waiting
    /// for the hotkey's modifiers to come up and the key presses run on a worker.
    fn deliver(&mut self, text: &str, target: isize, paste: bool) -> bool {
        if !clip::set_text(self.hwnd.hwnd(), text) {
            return false;
        }
        if paste {
            let own = self.hwnd.0;
            let _ = std::thread::Builder::new().name("voice-paste".into()).spawn(move || paste_into(target, own));
        }
        true
    }

    fn status(&self) -> (String, String) {
        let hk = if self.hotkey.trim().is_empty() { "the mic button".to_string() } else { self.hotkey.clone() };
        match self.phase {
            Phase::Recording => {
                let hint =
                    if self.pressed_at.is_some() { "Release to finish" } else { "Press again or click to finish" };
                (format!("Listening  {}", fmt_secs(self.started_at.elapsed())), format!("{hint} · {hk}"))
            }
            Phase::Transcribing => ("Transcribing…".into(), self.cfg.model.clone()),
            Phase::Idle => match &self.error {
                Some((title, e)) => (title.to_string(), e.clone()),
                None if dotenv::get(stt::KEY_VAR).is_none() => {
                    ("Dictation".into(), format!("Add {}=… to your .env file to enable it", stt::KEY_VAR))
                }
                None => ("Dictation".into(), format!("Tap {hk} to start and stop, or hold it while you talk")),
            },
        }
    }
}

fn fmt_secs(d: Duration) -> String {
    let s = d.as_secs();
    format!("{}:{:02}", s / 60, s % 60)
}

fn ago(d: Duration) -> String {
    let s = d.as_secs();
    match s {
        0..=59 => "now".into(),
        60..=3599 => format!("{}m", s / 60),
        _ => format!("{}h", s / 3600),
    }
}

fn key_input(vk: VIRTUAL_KEY, up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: 0,
                dwFlags: if up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

/// Ctrl+V into `target` once no modifier is held (otherwise Ctrl+V would turn
/// into Ctrl+Alt+V while the dictation hotkey is still down).
fn paste_into(target: isize, own: isize) {
    let held = || {
        [VK_CONTROL, VK_MENU, VK_SHIFT, VK_LWIN, VK_RWIN].iter().any(|k| unsafe { GetAsyncKeyState(k.0 as i32) } < 0)
    };
    let deadline = Instant::now() + Duration::from_secs(3);
    while held() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    unsafe {
        let t = HWND(target as *mut _);
        if target != 0 && GetForegroundWindow() != t && IsWindow(Some(t)).as_bool() {
            let _ = SetForegroundWindow(t);
            std::thread::sleep(Duration::from_millis(80));
        }
        if GetForegroundWindow().0 as isize == own {
            return; // nothing to paste into; the text is on the clipboard
        }
        let v = VIRTUAL_KEY(b'V' as u16);
        let inputs =
            [key_input(VK_CONTROL, false), key_input(v, false), key_input(v, true), key_input(VK_CONTROL, true)];
        SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
    }
}

impl Module for Voice {
    fn id(&self) -> ModuleId {
        ID
    }
    fn title(&self) -> &str {
        "Dictation"
    }
    fn icon(&self) -> Icon {
        Icon::Mic
    }

    fn start(&mut self, cx: &mut Cx) {
        self.hwnd = cx.hwnd;
        self.bus = Some(cx.bus.clone());
        self.apply_cfg(cx);
    }

    fn stop(&mut self) {
        self.cancel();
    }

    fn on_message(&mut self, msg: Box<dyn Any + Send>, cx: &mut Cx) {
        let Ok(m) = msg.downcast::<VoiceMsg>() else { return };
        match *m {
            VoiceMsg::Level(g, l) if g == self.gen && self.phase == Phase::Recording => {
                if self.levels.len() == LEVELS {
                    self.levels.pop_front();
                }
                self.levels.push_back(l);
            }
            VoiceMsg::Recorded(g, samples) if g == self.gen => {
                self.stop = None;
                let secs = samples.len() as f32 / audio::SAMPLE_RATE as f32;
                crate::log!("voice: recorded {secs:.1}s, rms {:.4}", audio::rms(&samples));
                if secs < MIN_SECONDS {
                    self.set_phase(Phase::Idle);
                } else if audio::rms(&samples) < MIN_RMS {
                    self.set_phase(Phase::Idle);
                    cx.fx.peek(
                        Peek::new(
                            Icon::Mic,
                            palette::ORANGE,
                            "Didn't hear anything",
                            "Check that the right microphone is the default",
                        )
                        .key("voice")
                        .page(ID),
                    );
                } else {
                    self.transcribe(samples);
                }
            }
            VoiceMsg::Transcribed(g, text) if g == self.gen => {
                self.set_phase(Phase::Idle);
                if text.trim().is_empty() {
                    cx.fx.peek(
                        Peek::new(Icon::Mic, palette::TEXT_DIM, "No speech detected", "")
                            .key("voice")
                            .duration_ms(1800),
                    );
                    return;
                }
                let paste = self.cfg.auto_paste;
                let ok = self.deliver(&text, self.target, paste);
                let title = match (ok, paste) {
                    (true, true) => "Pasted",
                    (true, false) => "Copied to clipboard",
                    (false, _) => "Couldn't use the clipboard",
                };
                cx.fx.peek(
                    Peek::new(Icon::Mic, palette::GREEN, title, truncate_chars(&one_line(&text), 120))
                        .key("voice")
                        .duration_ms(2600)
                        .page(ID),
                );
                self.history.insert(0, Entry { text, at: cx.now });
                self.history.truncate(HISTORY);
            }
            VoiceMsg::Failed(g, title, e) if g == self.gen => {
                crate::log!("voice: {title}: {e}");
                self.stop = None;
                self.set_phase(Phase::Idle);
                let color = if e == audio::MUTED { palette::ORANGE } else { palette::RED };
                cx.fx.peek(Peek::new(Icon::Mic, color, title, e.clone()).key("voice").duration_ms(4000).page(ID));
                self.error = Some((title, e));
            }
            _ => {}
        }
        cx.fx.redraw = true;
    }

    fn on_system(&mut self, ev: &SystemEvent, cx: &mut Cx) {
        match ev {
            SystemEvent::ConfigReloaded => self.apply_cfg(cx),
            SystemEvent::Command { verb, args } if verb == "dictate" => {
                match args.first().map(|a| a.to_ascii_lowercase()).as_deref() {
                    Some("start") => self.start(cx, false),
                    Some("stop") => self.finish(),
                    Some("cancel") => self.cancel(),
                    from => match self.phase {
                        Phase::Idle => self.start(cx, from == Some("hotkey")),
                        Phase::Recording => self.finish(),
                        Phase::Transcribing => {}
                    },
                }
                cx.fx.redraw = true;
            }
            SystemEvent::RawKey { vk, down: false } if *vk == self.dictate_vk && self.phase == Phase::Recording => {
                // push-to-talk: a long press ends with its release; a tap keeps recording
                if self.pressed_at.is_some_and(|t| cx.now - t >= HOLD) {
                    self.finish();
                    cx.fx.redraw = true;
                } else {
                    self.pressed_at = None;
                }
            }
            _ => {}
        }
    }

    fn activity(&self) -> Option<Activity> {
        let (left, right) = match self.phase {
            Phase::Recording => (
                Slot::Icon(Icon::Mic, REC_COLOR),
                Slot::Wave { levels: self.levels.iter().copied().collect(), color: REC_COLOR },
            ),
            Phase::Transcribing => (Slot::Ring(None, palette::PURPLE), Slot::Text("Writing".into(), palette::TEXT_DIM)),
            Phase::Idle => return None,
        };
        Some(Activity { priority: 90, left, right, wing: 2.1, page: Some(ID) })
    }

    fn has_page(&self) -> bool {
        true
    }

    fn page_height(&self, _w: f32) -> f32 {
        let rows = self.history.len().min(MAX_ROWS);
        if rows == 0 {
            64.0
        } else {
            64.0 + 28.0 + rows as f32 * (ROW_H + 4.0)
        }
    }

    fn draw_page(&mut self, ui: &mut Ui, r: Rect) {
        // --- status row: mic button, status text, live meter -----------------
        let btn = Rect::new(r.x, r.y + 6.0, 48.0, 48.0);
        let recording = self.phase == Phase::Recording;
        let (bg, fg) = match self.phase {
            Phase::Recording => (REC_COLOR, Color::white(1.0)),
            Phase::Transcribing => (palette::PURPLE.with_a(0.25), palette::PURPLE),
            Phase::Idle => (Color::white(0.1), palette::TEXT),
        };
        let icon = if recording { Icon::Stop } else { Icon::Mic };
        if ui.icon_button(btn, icon, ButtonStyle::filled(bg).fg(fg).scale(0.46)) {
            let mut fx = std::mem::take(ui.fx);
            {
                let mut cx = Cx { bus: ui.bus, cfg: ui.cfg, fx: &mut fx, hwnd: self.hwnd, now: ui.now };
                match self.phase {
                    Phase::Idle => self.start(&mut cx, false),
                    Phase::Recording => self.finish(),
                    Phase::Transcribing => {}
                }
            }
            *ui.fx = fx;
        }
        if self.phase == Phase::Transcribing {
            let t = crate::ui::epoch_secs() * 5.0;
            ui.p.arc(btn.cx(), btn.cy(), btn.w * 0.5 + 2.0, t, 1.4, 2.0, palette::PURPLE);
            ui.animate();
        }
        let (title, sub) = self.status();
        let tx = btn.right() + 14.0;
        let meter_w = if recording { 150.0 } else { 0.0 };
        let tw = r.right() - tx - meter_w - 8.0;
        let title_color = if self.error.is_some() && self.phase == Phase::Idle { palette::RED } else { palette::TEXT };
        ui.p.text(&title, Rect::new(tx, btn.y + 4.0, tw, 22.0), &TextStyle::new(15.0, title_color).semibold());
        ui.p.text(&sub, Rect::new(tx, btn.y + 26.0, tw, 18.0), &TextStyle::new(12.0, palette::TEXT_DIM));
        if recording {
            let mr = Rect::new(r.right() - meter_w, btn.y + 8.0, meter_w - 34.0, 32.0);
            let levels: Vec<f32> = self.levels.iter().copied().collect();
            draw_wave(ui, mr, &levels, REC_COLOR);
            let x = Rect::new(r.right() - 28.0, btn.cy() - 13.0, 26.0, 26.0);
            if ui.icon_button(x, Icon::Close, ButtonStyle::filled(Color::white(0.1)).scale(0.5).fg(palette::TEXT_DIM)) {
                self.cancel();
            }
        }

        // --- recent transcripts ------------------------------------------------
        if self.history.is_empty() {
            return;
        }
        let top = r.y + 64.0;
        ui.caption("RECENT", Rect::new(r.x, top, 200.0, 24.0));
        let copied = self.copied.filter(|c| c.1.elapsed() < Duration::from_millis(1500)).map(|c| c.0);
        let mut action: Option<(usize, bool)> = None;
        let mut y = top + 28.0;
        for (i, e) in self.history.iter().enumerate().take(MAX_ROWS) {
            let row = Rect::new(r.x, y, r.w, ROW_H);
            ui.p.fill_rounded(row, 10.0, palette::CARD);
            let paste = Rect::new(row.right() - 34.0, row.cy() - 13.0, 26.0, 26.0);
            let copy = Rect::new(paste.x - 30.0, row.cy() - 13.0, 26.0, 26.0);
            let age = Rect::new(copy.x - 44.0, row.y, 38.0, row.h);
            ui.p.text(&ago(e.at.elapsed()), age, &TextStyle::new(11.0, palette::TEXT_FAINT).right());
            ui.p.text(
                &one_line(&e.text),
                Rect::new(row.x + 12.0, row.y, age.x - row.x - 18.0, row.h),
                &TextStyle::new(12.5, palette::TEXT),
            );
            let done = copied == Some(i);
            let bs = ButtonStyle::default().scale(0.55);
            let (ci, cc) = if done { (Icon::Check, palette::GREEN) } else { (Icon::Copy, palette::TEXT_DIM) };
            if ui.icon_button(copy, ci, bs.fg(cc)) {
                action = Some((i, false));
            }
            if ui.icon_button(paste, Icon::Clipboard, bs.fg(palette::TEXT_DIM)) {
                action = Some((i, true));
            }
            y += ROW_H + 4.0;
        }
        if let Some((i, paste)) = action {
            let text = self.history[i].text.clone();
            // the notch never takes focus on click, so the foreground app is the target
            let fg = unsafe { GetForegroundWindow() }.0 as isize;
            if self.deliver(&text, fg, paste) && !paste {
                self.copied = Some((i, Instant::now()));
            }
        }
        if copied.is_some() {
            ui.animate();
        }
    }
}

/// Level meter: one rounded bar per recent level, newest on the right.
pub fn draw_wave(ui: &mut Ui, r: Rect, levels: &[f32], color: Color) {
    let (bw, gap) = (2.4, 2.0);
    let n = ((r.w + gap) / (bw + gap)).floor().max(1.0) as usize;
    let start = levels.len().saturating_sub(n);
    let shown = &levels[start..];
    let x0 = r.right() - shown.len() as f32 * (bw + gap) + gap;
    for (i, &l) in shown.iter().enumerate() {
        let h = (r.h * (0.12 + 0.88 * l)).max(2.4);
        let x = x0 + i as f32 * (bw + gap);
        ui.p.fill_rounded(Rect::new(x, r.cy() - h * 0.5, bw, h), bw * 0.5, color);
    }
    // idle dots for the part of the meter not filled yet
    let empty = n.saturating_sub(shown.len());
    for i in 0..empty {
        let x = r.x + i as f32 * (bw + gap);
        ui.p.fill_rounded(Rect::new(x, r.cy() - 1.2, bw, 2.4), 1.2, color.with_a(0.35));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formatting() {
        assert_eq!(fmt_secs(Duration::from_secs(7)), "0:07");
        assert_eq!(fmt_secs(Duration::from_secs(125)), "2:05");
        assert_eq!(ago(Duration::from_secs(10)), "now");
        assert_eq!(ago(Duration::from_secs(600)), "10m");
        assert_eq!(ago(Duration::from_secs(7200)), "2h");
    }
}
