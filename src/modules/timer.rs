//! Countdown timer + stopwatch. Ticks only once per second (10x while the
//! stopwatch page is visible), so a running timer costs next to nothing.

use std::time::{Duration, Instant};

use super::{Activity, CardSize, Cx, Module, ModuleId, Peek, Slot, SystemEvent, Trailing};
use crate::gfx::{Icon, TextStyle};
use crate::sys;
use crate::ui::{ButtonStyle, Ui};
use crate::util::{palette, Color, Rect};

const ID: ModuleId = "timer";
const TIMER_COLOR: Color = palette::ORANGE;
const SW_COLOR: Color = palette::TEAL;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Timer,
    Stopwatch,
}

pub struct Timer {
    mode: Mode,
    total: Duration,
    /// Set while the countdown runs.
    end: Option<Instant>,
    /// Remaining time while paused.
    paused_left: Option<Duration>,
    sw_start: Option<Instant>,
    sw_accum: Duration,
    presets: Vec<u32>,
    sound: bool,
    sound_volume: f32,
    expanded: bool,
}

impl Timer {
    pub fn new() -> Self {
        Self {
            mode: Mode::Timer,
            total: Duration::from_secs(5 * 60),
            end: None,
            paused_left: None,
            sw_start: None,
            sw_accum: Duration::ZERO,
            presets: vec![1, 3, 5, 10, 15, 25, 45],
            sound: true,
            sound_volume: 1.6,
            expanded: false,
        }
    }

    fn remaining(&self) -> Option<Duration> {
        match (self.end, self.paused_left) {
            (Some(e), _) => Some(e.saturating_duration_since(Instant::now())),
            (None, Some(p)) => Some(p),
            _ => None,
        }
    }

    fn timer_active(&self) -> bool {
        self.end.is_some() || self.paused_left.is_some()
    }

    fn sw_elapsed(&self) -> Duration {
        self.sw_accum + self.sw_start.map_or(Duration::ZERO, |s| s.elapsed())
    }

    fn sw_active(&self) -> bool {
        self.sw_start.is_some() || self.sw_accum > Duration::ZERO
    }

    fn start(&mut self, d: Duration) {
        self.total = d;
        self.end = Some(Instant::now() + d);
        self.paused_left = None;
        self.mode = Mode::Timer;
    }

    fn pause_resume(&mut self) {
        if let Some(e) = self.end.take() {
            self.paused_left = Some(e.saturating_duration_since(Instant::now()));
        } else if let Some(p) = self.paused_left.take() {
            self.end = Some(Instant::now() + p);
        } else {
            self.start(self.total);
        }
    }

    fn reset(&mut self) {
        self.end = None;
        self.paused_left = None;
    }

    fn sw_toggle(&mut self) {
        match self.sw_start.take() {
            Some(s) => self.sw_accum += s.elapsed(),
            None => self.sw_start = Some(Instant::now()),
        }
    }

    fn sw_reset(&mut self) {
        self.sw_start = None;
        self.sw_accum = Duration::ZERO;
    }

    fn progress(&self) -> f32 {
        match self.remaining() {
            Some(r) if !self.total.is_zero() => r.as_secs_f32() / self.total.as_secs_f32(),
            _ => 0.0,
        }
    }
}

fn fmt_countdown(d: Duration) -> String {
    let s = d.as_secs_f64().ceil() as u64;
    let (h, m, s) = (s / 3600, (s / 60) % 60, s % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

fn fmt_stopwatch(d: Duration, tenths: bool) -> String {
    let ms = d.as_millis() as u64;
    let (h, m, s, t) = (ms / 3_600_000, (ms / 60_000) % 60, (ms / 1000) % 60, (ms / 100) % 10);
    match (h > 0, tenths) {
        (true, _) => format!("{h}:{m:02}:{s:02}"),
        (false, true) => format!("{m}:{s:02}.{t}"),
        (false, false) => format!("{m}:{s:02}"),
    }
}

impl Module for Timer {
    fn id(&self) -> ModuleId {
        ID
    }
    fn title(&self) -> &str {
        "Timer"
    }
    fn icon(&self) -> Icon {
        Icon::Timer
    }

    fn start(&mut self, cx: &mut Cx) {
        self.presets = cx.cfg.timer.presets_min.clone();
        self.sound = cx.cfg.timer.sound;
        self.sound_volume = cx.cfg.timer.sound_volume;
    }

    fn on_system(&mut self, ev: &SystemEvent, cx: &mut Cx) {
        match ev {
            SystemEvent::Expanded(e) => self.expanded = *e,
            SystemEvent::ConfigReloaded => {
                self.presets = cx.cfg.timer.presets_min.clone();
                self.sound = cx.cfg.timer.sound;
                self.sound_volume = cx.cfg.timer.sound_volume;
            }
            SystemEvent::Command { verb, args } if verb == "timer" => {
                let mins = args.first().and_then(|a| a.parse::<f64>().ok()).unwrap_or(5.0);
                if mins <= 0.0 {
                    self.reset();
                } else {
                    self.start(Duration::from_secs_f64(mins * 60.0));
                    cx.fx.peek(
                        Peek::new(Icon::Timer, TIMER_COLOR, "Timer started", fmt_countdown(self.total))
                            .key("timer")
                            .duration_ms(1800),
                    );
                }
                cx.fx.redraw = true;
            }
            _ => {}
        }
    }

    fn next_tick(&self) -> Option<Instant> {
        let now = Instant::now();
        let mut next: Option<Instant> = None;
        if let Some(end) = self.end {
            // wake exactly when the displayed second changes
            let left = end.saturating_duration_since(now);
            let frac = left.as_secs_f64().fract();
            let wait = if frac < 0.001 { 1.0 } else { frac };
            next = Some(now + Duration::from_secs_f64(wait.min(left.as_secs_f64()).max(0.005)));
        }
        if self.sw_start.is_some() {
            let step = if self.expanded { 100 } else { 1000 };
            let t = now + Duration::from_millis(step);
            next = Some(next.map_or(t, |n| n.min(t)));
        }
        next
    }

    fn on_tick(&mut self, cx: &mut Cx) {
        if let Some(end) = self.end {
            if Instant::now() + Duration::from_millis(5) >= end {
                self.end = None;
                self.paused_left = None;
                crate::log!("timer done ({})", fmt_countdown(self.total));
                cx.fx.peek(
                    Peek::new(Icon::Bell, TIMER_COLOR, "Timer done", format!("{} elapsed", fmt_countdown(self.total)))
                        .trailing(Trailing::Text("0:00".into(), TIMER_COLOR))
                        .duration_ms(6000)
                        .key("timer")
                        .page(ID),
                );
                if self.sound {
                    sys::play_sound_gain("Notification.Reminder", self.sound_volume);
                }
            }
        }
        cx.fx.redraw = true;
    }

    fn activity(&self) -> Option<Activity> {
        if let Some(r) = self.remaining() {
            let paused = self.end.is_none();
            let c = if paused { palette::TEXT_DIM } else { TIMER_COLOR };
            return Some(Activity {
                priority: 40,
                left: Slot::Ring(Some(self.progress()), c),
                right: Slot::Text(fmt_countdown(r), c),
                wing: 1.75,
                page: Some(ID),
            });
        }
        if self.sw_start.is_some() {
            return Some(Activity {
                priority: 35,
                left: Slot::Icon(Icon::Timer, SW_COLOR),
                right: Slot::Text(fmt_stopwatch(self.sw_elapsed(), false), SW_COLOR),
                wing: 1.75,
                page: Some(ID),
            });
        }
        None
    }

    fn card(&self) -> Option<CardSize> {
        Some(CardSize::Small)
    }

    fn draw_card(&mut self, ui: &mut Ui, r: Rect) {
        if ui.card(r, true) {
            ui.fx.open_page = Some(ID);
        }
        let inner = r.inset_xy(12.0, 0.0);
        let is = 18.0;
        let sw = !self.timer_active() && self.sw_active();
        let color = if sw { SW_COLOR } else { TIMER_COLOR };
        ui.p.icon(Icon::Timer, Rect::new(inner.x, r.cy() - is * 0.5, is, is), color);
        let label_x = inner.x + is + 8.0;
        let bh = (r.h - 12.0).clamp(22.0, 30.0);
        if self.timer_active() || sw {
            let txt = if sw {
                fmt_stopwatch(self.sw_elapsed(), false)
            } else {
                fmt_countdown(self.remaining().unwrap_or_default())
            };
            let running = if sw { self.sw_start.is_some() } else { self.end.is_some() };
            let btn = Rect::new(inner.right() - bh, r.cy() - bh * 0.5, bh, bh);
            ui.p.text(
                &txt,
                Rect::new(label_x, r.y, btn.x - label_x - 6.0, r.h),
                &TextStyle::new(17.0, palette::TEXT).semibold().display(),
            );
            let icon = if running { Icon::Pause } else { Icon::Play };
            if ui.icon_button(btn, icon, ButtonStyle::filled(color.with_a(0.22)).fg(color).scale(0.46)) {
                if sw {
                    self.sw_toggle();
                } else {
                    self.pause_resume();
                }
            }
        } else {
            ui.p.text("Timer", Rect::new(label_x, r.y, 80.0, r.h), &TextStyle::new(13.0, palette::TEXT).medium());
            // quick starts
            let quick = [5u32, 25];
            let cw = 44.0;
            let mut x = inner.right() - (cw + 6.0) * quick.len() as f32 + 6.0;
            for m in quick {
                let b = Rect::new(x, r.cy() - bh * 0.5, cw, bh);
                if ui.pill_button(b, &format!("{m}m"), None, ButtonStyle::filled(Color::white(0.1))) {
                    self.start(Duration::from_secs(m as u64 * 60));
                }
                x += cw + 6.0;
            }
        }
    }

    fn has_page(&self) -> bool {
        true
    }

    fn page_height(&self, _w: f32) -> f32 {
        150.0
    }

    fn draw_page(&mut self, ui: &mut Ui, r: Rect) {
        // segmented mode switch
        let seg = Rect::new(r.x, r.y, 190.0, 28.0);
        ui.p.fill_rounded(seg, 14.0, Color::white(0.08));
        let half = Rect::new(seg.x, seg.y, seg.w * 0.5, seg.h);
        for (i, (label, m)) in [("Timer", Mode::Timer), ("Stopwatch", Mode::Stopwatch)].iter().enumerate() {
            let b = half.translate(i as f32 * half.w, 0.0).inset(2.0);
            let sel = self.mode == *m;
            if sel {
                ui.p.fill_rounded(b, 12.0, Color::white(0.16));
            }
            let st = TextStyle::new(12.0, if sel { palette::TEXT } else { palette::TEXT_DIM }).medium().center();
            ui.p.text(label, b, &st);
            if ui.clicked(b) {
                self.mode = *m;
            }
            if ui.hovered(b) {
                ui.hot = true;
            }
        }

        let left = Rect::new(r.x, r.y + 34.0, 270.0, r.h - 34.0);
        let right = Rect::new(r.x + 290.0, r.y + 34.0, r.w - 290.0, r.h - 34.0);
        let bh = 32.0;
        match self.mode {
            Mode::Timer => {
                let rem = self.remaining();
                let running = self.end.is_some();
                let shown = rem.unwrap_or(self.total);
                let color = if rem.is_some() { TIMER_COLOR } else { palette::TEXT };
                ui.p.text(
                    &fmt_countdown(shown),
                    Rect::new(left.x, left.y - 2.0, left.w, 56.0),
                    &TextStyle::new(48.0, color).semibold().display(),
                );
                ui.progress(
                    Rect::new(left.x, left.y + 58.0, 240.0, 4.0),
                    if rem.is_some() { self.progress() } else { 1.0 },
                    color.with_a(0.9),
                );
                let by = left.bottom() - bh;
                let main = Rect::new(left.x, by, 104.0, bh);
                let (label, icon) = if running {
                    ("Pause", Icon::Pause)
                } else if rem.is_some() {
                    ("Resume", Icon::Play)
                } else {
                    ("Start", Icon::Play)
                };
                if ui.pill_button(
                    main,
                    label,
                    Some(icon),
                    ButtonStyle::filled(TIMER_COLOR.with_a(0.24)).fg(TIMER_COLOR),
                ) {
                    self.pause_resume();
                }
                let reset = Rect::new(main.right() + 8.0, by, bh, bh);
                if ui.icon_button(reset, Icon::Refresh, ButtonStyle::filled(Color::white(0.1)).scale(0.44)) {
                    self.reset();
                }
                // +1 / -1 minute adjust
                let plus = Rect::new(reset.right() + 8.0, by, bh, bh);
                if ui.icon_button(plus, Icon::Plus, ButtonStyle::filled(Color::white(0.1)).scale(0.6)) {
                    let add = Duration::from_secs(60);
                    match (self.end, self.paused_left) {
                        (Some(e), _) => {
                            self.end = Some(e + add);
                            self.total += add;
                        }
                        (None, Some(p)) => {
                            self.paused_left = Some(p + add);
                            self.total += add;
                        }
                        _ => self.total += add,
                    }
                }
                let minus = Rect::new(plus.right() + 8.0, by, bh, bh);
                if ui.icon_button(minus, Icon::Minus, ButtonStyle::filled(Color::white(0.1)).scale(0.6))
                    && !self.timer_active()
                {
                    self.total = self.total.saturating_sub(Duration::from_secs(60)).max(Duration::from_secs(60));
                }

                // presets grid
                ui.caption("PRESETS", Rect::new(right.x, right.y - 4.0, right.w, 14.0));
                let cols = 4usize;
                let gap = 8.0;
                let cw = ((right.w - gap * (cols as f32 - 1.0)) / cols as f32).min(70.0);
                let ch = 30.0;
                for (i, m) in self.presets.clone().iter().take(8).enumerate() {
                    let (cxi, cyi) = ((i % cols) as f32, (i / cols) as f32);
                    let b = Rect::new(right.x + cxi * (cw + gap), right.y + 16.0 + cyi * (ch + gap), cw, ch);
                    let label = if *m >= 60 { format!("{}h", m / 60) } else { format!("{m} min") };
                    if ui.pill_button(b, &label, None, ButtonStyle::filled(Color::white(0.09))) {
                        self.start(Duration::from_secs(*m as u64 * 60));
                    }
                }
            }
            Mode::Stopwatch => {
                let running = self.sw_start.is_some();
                let color = if self.sw_active() { SW_COLOR } else { palette::TEXT };
                ui.p.text(
                    &fmt_stopwatch(self.sw_elapsed(), true),
                    Rect::new(left.x, left.y - 2.0, left.w + 100.0, 56.0),
                    &TextStyle::new(48.0, color).semibold().display(),
                );
                let by = left.bottom() - bh;
                let main = Rect::new(left.x, by, 104.0, bh);
                let (label, icon) = if running { ("Stop", Icon::Pause) } else { ("Start", Icon::Play) };
                if ui.pill_button(main, label, Some(icon), ButtonStyle::filled(SW_COLOR.with_a(0.22)).fg(SW_COLOR)) {
                    self.sw_toggle();
                }
                let reset = Rect::new(main.right() + 8.0, by, bh, bh);
                if ui.icon_button(reset, Icon::Refresh, ButtonStyle::filled(Color::white(0.1)).scale(0.44)) {
                    self.sw_reset();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formatting() {
        assert_eq!(fmt_countdown(Duration::from_millis(299_100)), "5:00");
        assert_eq!(fmt_countdown(Duration::from_secs(3725)), "1:02:05");
        assert_eq!(fmt_stopwatch(Duration::from_millis(83_450), true), "1:23.4");
    }
}
