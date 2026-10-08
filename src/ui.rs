//! Tiny immediate-mode UI layer on top of `Painter`.
//!
//! Modules describe their UI every frame; interaction is resolved against
//! the input snapshot (hover position, press and click positions).

use std::time::Instant;

use crate::bus::Bus;
use crate::config::Config;
use crate::gfx::{Icon, Painter, TextStyle};
use crate::modules::Effects;
use crate::util::{palette, Color, Rect};

#[derive(Default, Clone, Debug)]
pub struct Input {
    /// Cursor position (logical, window-relative) when over the notch.
    pub mouse: Option<(f32, f32)>,
    /// Press position while the left button is held.
    pub down: Option<(f32, f32)>,
    /// Completed click this frame: (press, release).
    pub click: Option<((f32, f32), (f32, f32))>,
    pub right_click: Option<(f32, f32)>,
    /// Wheel notches this frame (positive = up).
    pub wheel: f32,
}

#[allow(dead_code)] // fields are for modules to use
pub struct Ui<'p, 'a> {
    pub p: &'p mut Painter<'a>,
    pub input: &'p Input,
    pub fx: &'p mut Effects,
    pub cfg: &'p Config,
    pub bus: &'p Bus,
    pub now: Instant,
    /// Current accent color (album art driven or configured).
    pub accent: Color,
    /// Set when the cursor is over something interactive.
    pub hot: bool,
    /// Interaction is disabled while content is fading (prevents misclicks).
    pub interactive: bool,
}

#[derive(Clone, Copy)]
pub struct ButtonStyle {
    pub fg: Color,
    pub bg: Color,
    pub hover_bg: Color,
    pub radius: f32,
    pub icon_scale: f32,
}

impl Default for ButtonStyle {
    fn default() -> Self {
        Self { fg: palette::TEXT, bg: Color::white(0.0), hover_bg: Color::white(0.12), radius: 999.0, icon_scale: 0.62 }
    }
}

#[allow(dead_code)]
impl ButtonStyle {
    pub fn filled(bg: Color) -> Self {
        Self { bg, hover_bg: bg.lerp(Color::white(1.0), 0.15).with_a(bg.a.max(0.2)), ..Default::default() }
    }
    pub fn fg(mut self, c: Color) -> Self {
        self.fg = c;
        self
    }
    pub fn scale(mut self, s: f32) -> Self {
        self.icon_scale = s;
        self
    }
    pub fn radius(mut self, r: f32) -> Self {
        self.radius = r;
        self
    }
}

#[allow(dead_code)]
impl<'p, 'a> Ui<'p, 'a> {
    pub fn hovered(&self, r: Rect) -> bool {
        self.interactive && self.input.mouse.is_some_and(|(x, y)| r.contains(x, y))
    }

    pub fn pressed(&self, r: Rect) -> bool {
        self.interactive
            && self.input.down.is_some_and(|(x, y)| r.contains(x, y))
            && self.input.mouse.is_some_and(|(x, y)| r.contains(x, y))
    }

    pub fn clicked(&mut self, r: Rect) -> bool {
        if !self.interactive {
            return false;
        }
        let hit = self.input.click.is_some_and(|((px, py), (ux, uy))| r.contains(px, py) && r.contains(ux, uy));
        if hit {
            self.fx.redraw = true;
        }
        hit
    }

    /// Click position inside `r` as a 0..1 fraction of its width.
    pub fn clicked_at(&mut self, r: Rect) -> Option<f32> {
        if self.clicked(r) {
            self.input.click.map(|(_, (ux, _))| ((ux - r.x) / r.w).clamp(0.0, 1.0))
        } else {
            None
        }
    }

    pub fn right_clicked(&self, r: Rect) -> bool {
        self.interactive && self.input.right_click.is_some_and(|(x, y)| r.contains(x, y))
    }

    pub fn wheel(&self, r: Rect) -> f32 {
        if self.hovered(r) {
            self.input.wheel
        } else {
            0.0
        }
    }

    pub fn animate(&mut self) {
        self.fx.animate = true;
    }

    // -- widgets --------------------------------------------------------------

    /// Round icon button. Returns true when clicked.
    pub fn icon_button(&mut self, r: Rect, icon: Icon, style: ButtonStyle) -> bool {
        let hover = self.hovered(r);
        let pressed = self.pressed(r);
        if hover {
            self.hot = true;
        }
        let bg = if hover { style.hover_bg } else { style.bg };
        let rr = if pressed { r.inset(r.w * 0.04) } else { r };
        self.p.fill_rounded(rr, style.radius, bg);
        let s = r.w.min(r.h) * style.icon_scale * if pressed { 0.92 } else { 1.0 };
        self.p.icon(icon, r.center_square(s), style.fg);
        self.clicked(r)
    }

    /// Capsule button with a text label (and optional leading icon).
    pub fn pill_button(&mut self, r: Rect, label: &str, icon: Option<Icon>, style: ButtonStyle) -> bool {
        let hover = self.hovered(r);
        let pressed = self.pressed(r);
        if hover {
            self.hot = true;
        }
        let bg = if hover { style.hover_bg } else { style.bg };
        let rr = if pressed { r.inset(1.0) } else { r };
        self.p.fill_rounded(rr, style.radius.min(r.h * 0.5), bg);
        let ts = TextStyle::new((r.h * 0.42).clamp(10.0, 15.0), style.fg).medium().center();
        match icon {
            Some(ic) => {
                let (tw, _) = self.p.measure(label, &ts);
                let is = r.h * 0.5;
                let total = is + 6.0 + tw;
                let x = r.cx() - total * 0.5;
                self.p.icon(ic, Rect::new(x, r.cy() - is * 0.5, is, is), style.fg);
                let tr = Rect::new(x + is + 6.0, r.y, tw + 2.0, r.h);
                self.p.text(label, tr, &TextStyle { align: crate::gfx::Align::Start, ..ts });
            }
            None => self.p.text(label, r, &ts),
        }
        self.clicked(r)
    }

    /// Thin capsule progress bar.
    pub fn progress(&mut self, r: Rect, frac: f32, color: Color) {
        let rad = r.h * 0.5;
        self.p.fill_rounded(r, rad, Color::white(0.14));
        let f = frac.clamp(0.0, 1.0);
        if f > 0.0 {
            let w = (r.w * f).max(r.h);
            self.p.fill_rounded(Rect::new(r.x, r.y, w, r.h), rad, color);
        }
    }

    /// Indeterminate progress: a highlight sliding along the track.
    pub fn progress_indeterminate(&mut self, r: Rect, color: Color) {
        let rad = r.h * 0.5;
        self.p.fill_rounded(r, rad, Color::white(0.14));
        let t = (self.now.elapsed().as_secs_f32() + epoch_secs()) % 1.6 / 1.6;
        let seg = r.w * 0.32;
        let x = r.x - seg + (r.w + seg) * t;
        let l = x.max(r.x);
        let rgt = (x + seg).min(r.right());
        if rgt > l {
            self.p.fill_rounded(Rect::new(l, r.y, rgt - l, r.h), rad, color);
        }
        self.fx.animate = true;
    }

    /// Rounded card background, highlighted on hover when `interactive`.
    pub fn card(&mut self, r: Rect, interactive: bool) -> bool {
        let hover = interactive && self.hovered(r);
        if hover {
            self.hot = true;
        }
        self.p.fill_rounded(r, 16.0, if hover { palette::CARD_HOVER } else { palette::CARD });
        interactive && self.clicked(r)
    }

    pub fn label(&mut self, s: &str, r: Rect, style: TextStyle) {
        self.p.text(s, r, &style);
    }

    /// Section header: small dim caps text.
    pub fn caption(&mut self, s: &str, r: Rect) {
        self.p.text(s, r, &TextStyle::new(11.0, palette::TEXT_FAINT).semibold());
    }

    /// Empty-state placeholder: icon + message centered in `r`.
    pub fn empty_state(&mut self, r: Rect, icon: Icon, msg: &str) {
        let is = 26.0;
        let total = is + 8.0 + 18.0;
        let top = r.cy() - total * 0.5;
        self.p.icon(icon, Rect::new(r.cx() - is * 0.5, top, is, is), palette::TEXT_FAINT);
        self.p.text(msg, Rect::new(r.x, top + is + 8.0, r.w, 18.0), &TextStyle::new(12.5, palette::TEXT_DIM).center());
    }
}

/// Seconds since process start, used to keep looping animations continuous.
pub fn epoch_secs() -> f32 {
    use std::sync::OnceLock;
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_secs_f32()
}
