//! Quick AI prompt (Ctrl+Alt+Space by default) backed by OpenRouter.
//!
//! The API key is read from OPENROUTER_API_KEY (environment or a
//! git-ignored `.env` file) — never from config.toml. Responses stream in
//! over SSE on a background thread.

use std::any::Any;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::{Cx, Key, Module, ModuleId, SystemEvent};
use crate::anim::{ease_out, smoothstep};
use crate::bus::Bus;
use crate::config::AiCfg;
use crate::gfx::{Icon, TextStyle};
use crate::sys::{clip, dotenv, http};
use crate::ui::{ButtonStyle, Ui};
use crate::util::{palette, Color, Rect, SendHwnd};

const ID: ModuleId = "ai";
const INPUT_H: f32 = 40.0;
const FOOTER_H: f32 = 22.0;
const MAX_RESPONSE_H: f32 = 250.0;
const KEY_VAR: &str = "OPENROUTER_API_KEY";

enum AiMsg {
    Delta(u64, String),
    Done(u64),
    Error(u64, String),
}

#[derive(Clone)]
struct Turn {
    user: String,
    assistant: String,
}

pub struct Ai {
    input: Vec<char>,
    caret: usize,
    select_all: bool,
    history: Vec<Turn>,
    current: Option<Turn>,
    streaming: bool,
    error: Option<String>,
    gen: u64,
    cancel: Option<Arc<AtomicBool>>,
    scroll: f32,
    stick_bottom: bool,
    text_h: f32,
    /// When the current question was sent (loader clock, fade-up).
    asked_at: Instant,
    /// First streamed token of the current answer (caret fade-in).
    first_token_at: Option<Instant>,
    /// Answer finished or stopped (action buttons fade in).
    done_at: Option<Instant>,
    expanded: bool,
    last_draw: Option<Instant>,
    last_key: Instant,
    hwnd: SendHwnd,
    bus: Option<Bus>,
    cfg: AiCfg,
}

impl Ai {
    pub fn new() -> Self {
        Self {
            input: Vec::new(),
            caret: 0,
            select_all: false,
            history: Vec::new(),
            current: None,
            streaming: false,
            error: None,
            gen: 0,
            cancel: None,
            scroll: 0.0,
            stick_bottom: true,
            text_h: 0.0,
            asked_at: Instant::now(),
            first_token_at: None,
            done_at: None,
            expanded: false,
            last_draw: None,
            last_key: Instant::now(),
            hwnd: SendHwnd(0),
            bus: None,
            cfg: AiCfg::default(),
        }
    }

    fn input_text(&self) -> String {
        self.input.iter().collect()
    }

    fn insert(&mut self, s: &str) {
        if self.select_all {
            self.input.clear();
            self.caret = 0;
            self.select_all = false;
        }
        for c in s.chars().map(|c| if c == '\n' || c == '\r' || c == '\t' { ' ' } else { c }) {
            if self.input.len() < 4000 {
                self.input.insert(self.caret, c);
                self.caret += 1;
            }
        }
    }

    fn stop_stream(&mut self) {
        if let Some(c) = self.cancel.take() {
            c.store(true, Ordering::Relaxed);
        }
        if self.streaming {
            self.streaming = false;
            self.done_at = Some(Instant::now());
            if let Some(t) = self.current.take() {
                if !t.assistant.is_empty() {
                    self.history.push(t);
                }
            }
        }
    }

    fn new_chat(&mut self) {
        self.stop_stream();
        self.history.clear();
        self.current = None;
        self.error = None;
        self.scroll = 0.0;
    }

    fn submit(&mut self) {
        let prompt = self.input_text().trim().to_string();
        if prompt.is_empty() || self.streaming {
            return;
        }
        let Some(key) = dotenv::get(KEY_VAR) else {
            self.error = Some(format!("No API key — add {KEY_VAR}=… to a .env file (see .env.example)"));
            return;
        };
        let Some(bus) = self.bus.clone() else { return };
        if let Some(t) = self.current.take() {
            self.history.push(t);
        }
        self.error = None;
        self.input.clear();
        self.caret = 0;
        self.select_all = false;
        self.gen += 1;
        self.streaming = true;
        self.scroll = 0.0;
        self.stick_bottom = true;
        self.asked_at = Instant::now();
        self.first_token_at = None;
        self.done_at = None;

        let mut messages = vec![json!({ "role": "system", "content": self.cfg.system_prompt })];
        // keep the conversation short: last 6 turns
        for t in self.history.iter().rev().take(6).collect::<Vec<_>>().into_iter().rev() {
            messages.push(json!({ "role": "user", "content": t.user }));
            messages.push(json!({ "role": "assistant", "content": t.assistant }));
        }
        messages.push(json!({ "role": "user", "content": prompt }));
        self.current = Some(Turn { user: prompt, assistant: String::new() });

        let body = json!({
            "model": self.cfg.model,
            "messages": messages,
            "stream": true,
            "max_tokens": self.cfg.max_tokens,
        });
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel = Some(cancel.clone());
        let gen = self.gen;
        let endpoint = self.cfg.endpoint.clone();
        let _ = std::thread::Builder::new()
            .name("ai-request".into())
            .spawn(move || request(endpoint, key, body, gen, cancel, bus));
    }

    fn response_text(&self) -> Option<&Turn> {
        self.current.as_ref().or(self.history.last())
    }
}

fn request(endpoint: String, key: String, body: Value, gen: u64, cancel: Arc<AtomicBool>, bus: Bus) {
    let headers = [
        ("Authorization", format!("Bearer {key}")),
        ("Content-Type", "application/json".to_string()),
        ("Accept", "text/event-stream".to_string()),
        ("HTTP-Referer", "https://github.com/eigenes/DynamicNotch".to_string()),
        ("X-Title", "Dynamic Notch".to_string()),
    ];
    let payload = body.to_string().into_bytes();
    let mut parser = http::SseParser::default();
    let mut raw = Vec::new();
    let mut finished = false;
    let mut stream_error: Option<String> = None;
    let res = http::post_stream(&endpoint, &headers, &payload, &cancel, |chunk| {
        if raw.len() < 16 * 1024 {
            raw.extend_from_slice(chunk);
        }
        parser.push(chunk, |data| {
            if data == "[DONE]" {
                finished = true;
                return;
            }
            let Ok(v) = serde_json::from_str::<Value>(data) else { return };
            if let Some(e) = v.get("error") {
                stream_error = Some(e.get("message").and_then(Value::as_str).unwrap_or("stream error").to_string());
                return;
            }
            if let Some(t) = v.pointer("/choices/0/delta/content").and_then(Value::as_str) {
                if !t.is_empty() {
                    bus.to_module(ID, AiMsg::Delta(gen, t.to_string()));
                }
            }
        });
        !cancel.load(Ordering::Relaxed)
    });
    if cancel.load(Ordering::Relaxed) {
        return;
    }
    match res {
        Ok(200) if stream_error.is_none() => {
            let _ = finished;
            bus.to_module(ID, AiMsg::Done(gen));
        }
        Ok(status) => {
            let msg = stream_error.unwrap_or_else(|| {
                serde_json::from_slice::<Value>(&raw)
                    .ok()
                    .and_then(|v| v.pointer("/error/message").and_then(Value::as_str).map(str::to_string))
                    .unwrap_or_else(|| format!("HTTP {status}"))
            });
            let hint = match status {
                401 => " (check OPENROUTER_API_KEY)",
                402 => " (out of OpenRouter credits)",
                429 => " (rate limited, try again shortly)",
                _ => "",
            };
            bus.to_module(ID, AiMsg::Error(gen, format!("{msg}{hint}")));
        }
        Err(e) => bus.to_module(ID, AiMsg::Error(gen, e)),
    }
}

impl Module for Ai {
    fn id(&self) -> ModuleId {
        ID
    }
    fn title(&self) -> &str {
        "Ask AI"
    }
    fn icon(&self) -> Icon {
        Icon::Sparkle
    }

    fn start(&mut self, cx: &mut Cx) {
        self.hwnd = cx.hwnd;
        self.bus = Some(cx.bus.clone());
        self.cfg = cx.cfg.ai.clone();
    }

    fn stop(&mut self) {
        self.stop_stream();
    }

    fn on_message(&mut self, msg: Box<dyn Any + Send>, cx: &mut Cx) {
        let Ok(m) = msg.downcast::<AiMsg>() else { return };
        match *m {
            AiMsg::Delta(g, t) if g == self.gen => {
                if let Some(cur) = self.current.as_mut() {
                    cur.assistant.push_str(&t);
                }
                self.first_token_at.get_or_insert(cx.now);
            }
            AiMsg::Done(g) if g == self.gen => {
                self.streaming = false;
                self.cancel = None;
                self.done_at = Some(cx.now);
            }
            AiMsg::Error(g, e) if g == self.gen => {
                self.streaming = false;
                self.cancel = None;
                self.error = Some(e);
                if self.current.as_ref().is_some_and(|c| c.assistant.is_empty()) {
                    // restore the prompt so it can be retried
                    if let Some(c) = self.current.take() {
                        self.input = c.user.chars().collect();
                        self.caret = self.input.len();
                    }
                }
            }
            _ => {}
        }
        cx.fx.layout_changed = true;
    }

    fn on_system(&mut self, ev: &SystemEvent, cx: &mut Cx) {
        match ev {
            SystemEvent::Expanded(e) => self.expanded = *e,
            SystemEvent::ConfigReloaded => self.cfg = cx.cfg.ai.clone(),
            SystemEvent::Command { verb, args } if verb == "ask" => {
                if !args.is_empty() {
                    self.input = args.join(" ").chars().collect();
                    self.caret = self.input.len();
                    self.select_all = false;
                    self.submit();
                }
                cx.fx.open_page = Some(ID);
                cx.fx.layout_changed = true;
            }
            _ => {}
        }
    }

    fn next_tick(&self) -> Option<Instant> {
        // caret blink while the page is on screen; the loader and fades
        // request their own frames from draw_page while they run
        let visible = self.expanded && self.last_draw.is_some_and(|t| t.elapsed() < Duration::from_millis(800));
        (visible && !self.streaming).then(|| Instant::now() + Duration::from_millis(530))
    }

    fn on_tick(&mut self, cx: &mut Cx) {
        cx.fx.redraw = true;
    }

    fn has_page(&self) -> bool {
        true
    }

    fn wants_keyboard(&self) -> bool {
        true
    }

    fn page_height(&self, _w: f32) -> f32 {
        let body = if self.response_text().is_some() || self.error.is_some() {
            self.text_h.clamp(24.0, MAX_RESPONSE_H) + 30.0
        } else {
            34.0
        };
        INPUT_H + 10.0 + body + FOOTER_H
    }

    fn on_key(&mut self, key: Key, cx: &mut Cx) {
        self.last_key = Instant::now();
        match key {
            Key::Char(c) => {
                let mut b = [0u8; 4];
                self.insert(c.encode_utf8(&mut b));
            }
            Key::Backspace => {
                if self.select_all {
                    self.input.clear();
                    self.caret = 0;
                    self.select_all = false;
                } else if self.caret > 0 {
                    self.caret -= 1;
                    self.input.remove(self.caret);
                }
            }
            Key::Delete => {
                if self.select_all {
                    self.input.clear();
                    self.caret = 0;
                    self.select_all = false;
                } else if self.caret < self.input.len() {
                    self.input.remove(self.caret);
                }
            }
            Key::Left => {
                self.select_all = false;
                self.caret = self.caret.saturating_sub(1);
            }
            Key::Right => {
                self.select_all = false;
                self.caret = (self.caret + 1).min(self.input.len());
            }
            Key::Home => {
                self.select_all = false;
                self.caret = 0;
            }
            Key::End => {
                self.select_all = false;
                self.caret = self.input.len();
            }
            Key::Up => {
                self.scroll = (self.scroll - 40.0).max(0.0);
                self.stick_bottom = false;
            }
            Key::Down => self.scroll += 40.0,
            Key::Enter { .. } => self.submit(),
            Key::SelectAll => self.select_all = !self.input.is_empty(),
            Key::Paste => {
                if let Some(t) = clip::get_text(self.hwnd.hwnd()) {
                    self.insert(&t);
                }
            }
            Key::Copy | Key::Cut => {
                if self.select_all && !self.input.is_empty() {
                    clip::set_text(self.hwnd.hwnd(), &self.input_text());
                    if key == Key::Cut {
                        self.input.clear();
                        self.caret = 0;
                        self.select_all = false;
                    }
                } else if let Some(t) = self.response_text() {
                    clip::set_text(self.hwnd.hwnd(), &t.assistant.clone());
                }
            }
            Key::Tab | Key::Escape => {}
        }
        cx.fx.layout_changed = true;
    }

    fn draw_page(&mut self, ui: &mut Ui, r: Rect) {
        self.last_draw = Some(Instant::now());
        let focused = ui.input.mouse.is_some() || self.last_key.elapsed() < Duration::from_secs(30);

        // --- input field ---------------------------------------------------
        let field = Rect::new(r.x, r.y, r.w, INPUT_H);
        let accent = palette::PURPLE;
        ui.p.fill_rounded(field, 14.0, Color::white(0.08));
        ui.p.stroke_rounded(field.inset(0.5), 14.0, 1.0, accent.with_a(if focused { 0.55 } else { 0.18 }));
        if ui.clicked(field) {
            ui.fx.keyboard_focus = Some(true);
        }
        if ui.hovered(field) {
            ui.hot = true;
        }
        let ic = Rect::new(field.x + 12.0, field.cy() - 9.0, 18.0, 18.0);
        ui.p.icon(Icon::Sparkle, ic, accent);
        let btn = Rect::new(field.right() - 34.0, field.cy() - 14.0, 28.0, 28.0);
        let text_r = Rect::new(ic.right() + 10.0, field.y, btn.x - ic.right() - 16.0, field.h);
        let st = TextStyle::new(14.0, palette::TEXT);
        if self.input.is_empty() {
            let hint = if self.streaming { "Answering…" } else { "Ask anything…" };
            ui.p.text(hint, text_r, &st.color(palette::TEXT_FAINT));
        }
        // horizontal scroll so the caret stays visible
        let before: String = self.input[..self.caret].iter().collect();
        let (caret_x, _) = ui.p.measure(&before, &st);
        let shift = (caret_x - text_r.w + 4.0).max(0.0);
        ui.p.push_clip(text_r);
        if !self.input.is_empty() {
            let all = self.input_text();
            let (tw, _) = ui.p.measure(&all, &st);
            if self.select_all {
                ui.p.fill_rounded(Rect::new(text_r.x - shift, field.cy() - 10.0, tw, 20.0), 4.0, accent.with_a(0.35));
            }
            ui.p.text(&all, Rect::new(text_r.x - shift, text_r.y, tw + 20.0, text_r.h), &st);
        }
        let blink = (self.last_key.elapsed().as_millis() / 530).is_multiple_of(2);
        if focused && blink && !self.streaming {
            let x = text_r.x + caret_x - shift;
            ui.p.fill_rect(Rect::new(x, field.cy() - 9.0, 1.6, 18.0), accent);
        }
        ui.p.pop_clip();
        if self.streaming {
            if ui.icon_button(btn, Icon::Stop, ButtonStyle::filled(Color::white(0.14)).scale(0.5)) {
                self.stop_stream();
            }
        } else {
            let can = !self.input.is_empty();
            let style = if can {
                ButtonStyle::filled(accent.with_a(0.85))
            } else {
                ButtonStyle::filled(Color::white(0.08)).fg(palette::TEXT_FAINT)
            };
            if ui.icon_button(btn, Icon::Send, style.scale(0.48)) && can {
                self.submit();
            }
        }

        // --- response ------------------------------------------------------
        let body_top = field.bottom() + 10.0;
        let footer = Rect::new(r.x, r.bottom() - FOOTER_H, r.w, FOOTER_H);
        let area = Rect::new(r.x, body_top, r.w, (footer.y - body_top - 4.0).max(0.0));
        if let Some(err) = self.error.clone() {
            let es = TextStyle::new(12.5, palette::RED).wrap().top();
            self.text_h = ui.p.measure_wrapped(&err, &es, area.w - 8.0);
            ui.p.text(&err, Rect::new(area.x + 4.0, area.y + 4.0, area.w - 8.0, area.h), &es);
        } else if let Some(turn) = self.response_text().cloned() {
            // a freshly asked question fades up into place
            let reveal = ease_out(ui.now.saturating_duration_since(self.asked_at).as_secs_f32() / 0.32);
            let alpha = ui.p.alpha;
            ui.p.alpha = alpha * reveal;
            let area = area.translate(0.0, 8.0 * (1.0 - reveal));
            let q = TextStyle::new(12.0, palette::TEXT_FAINT);
            ui.p.text(&format!("› {}", turn.user), Rect::new(area.x + 4.0, area.y, area.w - 8.0, 18.0), &q);
            let resp_area = Rect::new(area.x + 4.0, area.y + 24.0, area.w - 8.0, (area.h - 24.0).max(0.0));
            if turn.assistant.is_empty() && self.streaming {
                self.text_h = 20.0;
                let t = ui.now.saturating_duration_since(self.asked_at).as_secs_f32();
                draw_loader(ui, Rect::new(resp_area.x, resp_area.y, resp_area.w, 20.0), t, accent);
                ui.animate();
            } else {
                let rs = TextStyle::new(13.5, palette::TEXT).wrap().top();
                let layout = ui.p.layout(&turn.assistant, &rs, resp_area.w, 100_000.0);
                let h = layout.as_ref().map_or(0.0, |l| l.height);
                self.text_h = h;
                let max_scroll = (h - resp_area.h).max(0.0);
                let wheel = ui.wheel(resp_area);
                if wheel != 0.0 {
                    self.scroll -= wheel * 40.0;
                    self.stick_bottom = self.scroll >= max_scroll - 1.0;
                }
                if self.stick_bottom && self.streaming {
                    self.scroll = max_scroll;
                }
                self.scroll = self.scroll.clamp(0.0, max_scroll);
                let ty = resp_area.y - self.scroll;
                ui.p.push_clip(resp_area);
                if let Some(l) = &layout {
                    ui.p.draw_layout(l, resp_area.x, ty, rs.color);
                    // streaming caret: a thin bar after the last token that fades in
                    if let (true, Some(t0)) = (self.streaming, self.first_token_at) {
                        let k = ease_out(ui.now.saturating_duration_since(t0).as_secs_f32() / 0.15);
                        let (cx, ly, lh) = l.caret(turn.assistant.encode_utf16().count() as u32);
                        let ch = 13.0;
                        let bar = Rect::new(resp_area.x + cx + 2.0, ty + ly + (lh - ch) * 0.5 + 1.0, 2.0, ch);
                        ui.p.fill_rounded(bar, 1.0, palette::TEXT.alpha(k));
                        if k < 1.0 {
                            ui.animate();
                        }
                    }
                }
                ui.p.pop_clip();
                if max_scroll > 0.0 {
                    // scroll indicator
                    let th = (resp_area.h * resp_area.h / h).max(16.0);
                    let ty = resp_area.y + (resp_area.h - th) * (self.scroll / max_scroll);
                    ui.p.fill_rounded(Rect::new(resp_area.right() + 1.0, ty, 3.0, th), 1.5, Color::white(0.25));
                }
            }
            ui.p.alpha = alpha;
            if reveal < 1.0 {
                ui.animate();
            }
        } else {
            self.text_h = 0.0;
            let hint = if dotenv::get(KEY_VAR).is_some() {
                "Enter to send · Esc to close · Ctrl+V to paste"
            } else {
                "Add OPENROUTER_API_KEY to a .env file to enable AI"
            };
            ui.p.text(hint, Rect::new(area.x + 4.0, area.y, area.w, 22.0), &TextStyle::new(12.0, palette::TEXT_FAINT));
        }

        // --- footer ----------------------------------------------------------
        ui.p.text(
            &self.cfg.model,
            Rect::new(footer.x + 4.0, footer.y, footer.w * 0.6, footer.h),
            &TextStyle::new(11.0, palette::TEXT_FAINT),
        );
        let has_chat = !self.history.is_empty() || self.current.is_some();
        if has_chat {
            let nb = Rect::new(footer.right() - 92.0, footer.y, 92.0, footer.h);
            if ui.pill_button(
                nb,
                "New chat",
                Some(Icon::Refresh),
                ButtonStyle::filled(Color::white(0.08)).fg(palette::TEXT_DIM),
            ) {
                self.new_chat();
                ui.fx.layout_changed = true;
            }
            if let Some(t) = self.response_text() {
                if !t.assistant.is_empty() && !self.streaming {
                    let cb = Rect::new(nb.x - 30.0, footer.y, footer.h, footer.h);
                    let text = t.assistant.clone();
                    // actions fade in once the answer is complete
                    let k =
                        self.done_at.map_or(1.0, |d| ease_out(ui.now.saturating_duration_since(d).as_secs_f32() / 0.4));
                    let alpha = ui.p.alpha;
                    ui.p.alpha = alpha * k;
                    if ui.icon_button(cb, Icon::Copy, ButtonStyle::default().scale(0.62).fg(palette::TEXT_DIM)) {
                        clip::set_text(self.hwnd.hwnd(), &text);
                    }
                    ui.p.alpha = alpha;
                    if k < 1.0 {
                        ui.animate();
                    }
                }
            }
        }
    }
}

/// Shown until the first token arrives: a 3×3 pixel grid whose cells pulse
/// in a staggered sweep, a shimmering label and the elapsed time.
fn draw_loader(ui: &mut Ui, r: Rect, t: f32, color: Color) {
    // per-cell start offsets (row-major); the lit cells sweep left → right
    const DELAYS: [f32; 9] = [0.09, 0.18, 0.27, 0.0, 0.09, 0.18, 0.09, 0.18, 0.27];
    const PERIOD: f32 = 0.65;
    const PX: f32 = 4.0;
    const GAP: f32 = 1.5;
    let grid = 3.0 * PX + 2.0 * GAP;
    let gy = r.cy() - grid * 0.5;
    for (i, d) in DELAYS.iter().enumerate() {
        let p = ((t - d) / PERIOD).rem_euclid(1.0);
        let on = if p < 0.42 { smoothstep(0.0, 0.18, p) } else { 1.0 - smoothstep(0.42, 0.62, p) };
        let cell = Rect::new(r.x + (i % 3) as f32 * (PX + GAP), gy + (i / 3) as f32 * (PX + GAP), PX, PX);
        ui.p.fill_rounded(cell, 1.0, color.alpha(0.15 + 0.85 * on));
    }
    let label = "Thinking";
    let st = TextStyle::new(13.0, palette::TEXT_FAINT).medium();
    let (lw, _) = ui.p.measure(label, &st);
    let lx = r.x + grid + 10.0;
    // the highlight travels from half a label-width before to half after, every 1.4 s
    let sweep = (t / 1.4).fract();
    ui.p.text_shimmer(
        label,
        Rect::new(lx, r.y, lw + 2.0, r.h),
        &st,
        palette::TEXT,
        lx + lw * (2.0 * sweep - 0.5),
        lw * 0.3,
    );
    let elapsed = TextStyle::new(12.0, palette::TEXT_FAINT).mono();
    ui.p.text(&format!("{t:.1}s"), Rect::new(lx + lw + 10.0, r.y, 60.0, r.h), &elapsed);
}
