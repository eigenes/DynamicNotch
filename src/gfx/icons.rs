//! Icon set. Media transport and a few symbols are drawn as vector paths
//! (crisp at any DPI, filled "SF Symbols"-like look); the rest come from the
//! built-in Segoe Fluent Icons font so no assets ship with the binary.

use super::{q, uq, Painter};
use crate::util::{Color, Rect};

#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Icon {
    // vector
    Play,
    Pause,
    Next,
    Prev,
    Sparkle,
    Check,
    Close,
    Plus,
    Minus,
    Stop,
    // glyphs
    Music,
    Timer,
    Clipboard,
    Download,
    Mic,
    Camera,
    Bell,
    Home,
    Settings,
    Folder,
    Copy,
    Trash,
    Image,
    File,
    Link,
    Refresh,
    Send,
    Info,
    Bolt,
    Globe,
    Keyboard,
    Package,
    Volume,
    Mute,
    Brightness,
    Headphones,
    Bluetooth,
    Mouse,
    Gamepad,
    Phone,
    Pulse,
}

impl Icon {
    fn glyph(self) -> Option<char> {
        Some(match self {
            Icon::Music => '\u{EC4F}',
            Icon::Timer => '\u{E916}',
            Icon::Clipboard => '\u{E77F}',
            Icon::Download => '\u{E896}',
            Icon::Mic => '\u{E720}',
            Icon::Camera => '\u{E714}',
            Icon::Bell => '\u{EA8F}',
            Icon::Home => '\u{E80F}',
            Icon::Settings => '\u{E713}',
            Icon::Folder => '\u{E8B7}',
            Icon::Copy => '\u{E8C8}',
            Icon::Trash => '\u{E74D}',
            Icon::Image => '\u{EB9F}',
            Icon::File => '\u{E8A5}',
            Icon::Link => '\u{E71B}',
            Icon::Refresh => '\u{E72C}',
            Icon::Send => '\u{E724}',
            Icon::Info => '\u{E946}',
            Icon::Bolt => '\u{E945}',
            Icon::Globe => '\u{E774}',
            Icon::Keyboard => '\u{E765}',
            Icon::Package => '\u{E7B8}',
            Icon::Volume => '\u{E767}',
            Icon::Mute => '\u{E74F}',
            Icon::Brightness => '\u{E706}',
            Icon::Headphones => '\u{E7F6}',
            Icon::Bluetooth => '\u{E702}',
            Icon::Mouse => '\u{E962}',
            Icon::Gamepad => '\u{E7FC}',
            Icon::Phone => '\u{E8EA}',
            Icon::Pulse => '\u{E9D9}',
            _ => return None,
        })
    }
}

pub fn draw(p: &mut Painter, icon: Icon, r: Rect, c: Color) {
    if let Some(ch) = icon.glyph() {
        let size = r.w.min(r.h) * 0.78;
        p.glyph(ch, r, size, c);
        return;
    }
    let s = r.w.min(r.h);
    let b = r.center_square(s);
    let (x, y) = (b.x, b.y);
    match icon {
        // Vector icons are built at the origin and cached on the GPU per size.
        Icon::Play => {
            let s = uq(q(s));
            p.fill_cached((10, q(s), 0, 0, 0), x, y, c, |pb| {
                let pts = [(s * 0.27, s * 0.16), (s * 0.27, s * 0.84), (s * 0.85, s * 0.50)];
                pb.rounded_poly(&pts, s * 0.09)
            });
        }
        Icon::Pause => {
            let w = s * 0.17;
            let h = s * 0.60;
            let gap = s * 0.15;
            let top = y + (s - h) * 0.5;
            let left = x + (s - (2.0 * w + gap)) * 0.5;
            p.fill_rounded(Rect::new(left, top, w, h), w * 0.32, c);
            p.fill_rounded(Rect::new(left + w + gap, top, w, h), w * 0.32, c);
        }
        Icon::Stop => {
            let q = s * 0.5;
            p.fill_rounded(b.center_square(q), q * 0.18, c);
        }
        Icon::Next | Icon::Prev => {
            let flip = icon == Icon::Prev;
            let s = uq(q(s));
            let kind = if flip { 12 } else { 11 };
            p.fill_cached((kind, q(s), 0, 0, 0), x, y, c, |pb| {
                for (x0, x1) in [(s * 0.07, s * 0.52), (s * 0.46, s * 0.91)] {
                    let (x0, x1) = if flip { (s - x0, s - x1) } else { (x0, x1) };
                    pb.rounded_poly(&[(x0, s * 0.23), (x0, s * 0.77), (x1, s * 0.50)], s * 0.06);
                }
            });
        }
        Icon::Sparkle => {
            let s = uq(q(s));
            p.fill_cached((13, q(s), 0, 0, 0), x, y, c, |pb| {
                for (cx, cy, rad) in [(s * 0.42, s * 0.56, s * 0.40), (s * 0.80, s * 0.20, s * 0.17)] {
                    pb.move_to(cx, cy - rad);
                    pb.quad_to(cx + rad * 0.12, cy - rad * 0.12, cx + rad, cy);
                    pb.quad_to(cx + rad * 0.12, cy + rad * 0.12, cx, cy + rad);
                    pb.quad_to(cx - rad * 0.12, cy + rad * 0.12, cx - rad, cy);
                    pb.quad_to(cx - rad * 0.12, cy - rad * 0.12, cx, cy - rad);
                    pb.close();
                }
            });
        }
        Icon::Check => {
            let w = s * 0.10;
            p.line(x + s * 0.22, y + s * 0.52, x + s * 0.42, y + s * 0.72, w, c);
            p.line(x + s * 0.42, y + s * 0.72, x + s * 0.80, y + s * 0.30, w, c);
        }
        Icon::Close => {
            let w = s * 0.09;
            p.line(x + s * 0.28, y + s * 0.28, x + s * 0.72, y + s * 0.72, w, c);
            p.line(x + s * 0.72, y + s * 0.28, x + s * 0.28, y + s * 0.72, w, c);
        }
        Icon::Plus => {
            let w = s * 0.09;
            p.line(x + s * 0.5, y + s * 0.24, x + s * 0.5, y + s * 0.76, w, c);
            p.line(x + s * 0.24, y + s * 0.5, x + s * 0.76, y + s * 0.5, w, c);
        }
        Icon::Minus => {
            let w = s * 0.09;
            p.line(x + s * 0.24, y + s * 0.5, x + s * 0.76, y + s * 0.5, w, c);
        }
        _ => {}
    }
}

/// Battery glyph drawn as vector so the fill level is exact.
pub fn battery(p: &mut Painter, r: Rect, level: f32, charging: bool, fill: Color) {
    // body occupies ~88% width, cap the rest
    let h = r.h.min(r.w * 0.5);
    let w = h * 2.05;
    let body = Rect::new(r.cx() - w * 0.5, r.cy() - h * 0.5, w * 0.9, h);
    let stroke = (h * 0.09).max(1.0);
    p.stroke_rounded(body.inset(stroke * 0.5), h * 0.3, stroke, Color::white(0.45));
    let cap = Rect::new(body.right() + stroke * 0.6, body.cy() - h * 0.18, w * 0.06, h * 0.36);
    p.fill_rounded(cap, cap.w * 0.5, Color::white(0.45));
    let inner = body.inset(stroke * 2.0);
    let lw = (inner.w * level.clamp(0.0, 1.0)).max(inner.h * 0.3);
    p.fill_rounded(Rect::new(inner.x, inner.y, lw, inner.h), h * 0.16, fill);
    if charging {
        let bx = body.cx();
        let by = body.cy();
        // a slightly larger dark bolt underneath acts as the outline
        let bolt = |pb: &crate::gfx::PathBuilder, k: f32| {
            pb.move_to(k * 0.15, -k);
            pb.line_to(-k * 0.55, k * 0.12);
            pb.line_to(-k * 0.02, k * 0.12);
            pb.line_to(-k * 0.15, k);
            pb.line_to(k * 0.55, -k * 0.12);
            pb.line_to(k * 0.02, -k * 0.12);
            pb.close();
        };
        let k = uq(q(h * 0.42));
        p.fill_cached((14, q(k), 0, 0, 0), bx, by, Color::black(0.85), |pb| bolt(pb, k * 1.32));
        p.fill_cached((15, q(k), 0, 0, 0), bx, by, Color::white(1.0), |pb| bolt(pb, k));
    }
}
