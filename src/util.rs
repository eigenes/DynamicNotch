//! Small shared helpers: geometry, colors, wide strings, logging, paths.

use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

use windows::Win32::Foundation::HWND;

// ---------------------------------------------------------------------------
// Geometry
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

#[allow(dead_code)]
impl Rect {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }
    pub fn from_ltrb(l: f32, t: f32, r: f32, b: f32) -> Self {
        Self { x: l, y: t, w: r - l, h: b - t }
    }
    pub fn right(&self) -> f32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }
    pub fn cx(&self) -> f32 {
        self.x + self.w * 0.5
    }
    pub fn cy(&self) -> f32 {
        self.y + self.h * 0.5
    }
    pub fn contains(&self, px: f32, py: f32) -> bool {
        px >= self.x && px < self.x + self.w && py >= self.y && py < self.y + self.h
    }
    pub fn inset(&self, d: f32) -> Rect {
        Rect::new(self.x + d, self.y + d, (self.w - 2.0 * d).max(0.0), (self.h - 2.0 * d).max(0.0))
    }
    pub fn inset_xy(&self, dx: f32, dy: f32) -> Rect {
        Rect::new(self.x + dx, self.y + dy, (self.w - 2.0 * dx).max(0.0), (self.h - 2.0 * dy).max(0.0))
    }
    pub fn expand(&self, d: f32) -> Rect {
        Rect::new(self.x - d, self.y - d, self.w + 2.0 * d, self.h + 2.0 * d)
    }
    pub fn translate(&self, dx: f32, dy: f32) -> Rect {
        Rect::new(self.x + dx, self.y + dy, self.w, self.h)
    }
    /// Take `w` from the left, returning (taken, rest).
    pub fn split_left(&self, w: f32, gap: f32) -> (Rect, Rect) {
        let w = w.min(self.w);
        (Rect::new(self.x, self.y, w, self.h), Rect::new(self.x + w + gap, self.y, (self.w - w - gap).max(0.0), self.h))
    }
    pub fn split_right(&self, w: f32, gap: f32) -> (Rect, Rect) {
        let w = w.min(self.w);
        (Rect::new(self.right() - w, self.y, w, self.h), Rect::new(self.x, self.y, (self.w - w - gap).max(0.0), self.h))
    }
    pub fn split_top(&self, h: f32, gap: f32) -> (Rect, Rect) {
        let h = h.min(self.h);
        (Rect::new(self.x, self.y, self.w, h), Rect::new(self.x, self.y + h + gap, self.w, (self.h - h - gap).max(0.0)))
    }
    pub fn split_bottom(&self, h: f32, gap: f32) -> (Rect, Rect) {
        let h = h.min(self.h);
        (
            Rect::new(self.x, self.bottom() - h, self.w, h),
            Rect::new(self.x, self.y, self.w, (self.h - h - gap).max(0.0)),
        )
    }
    pub fn union(&self, o: &Rect) -> Rect {
        let l = self.x.min(o.x);
        let t = self.y.min(o.y);
        let r = self.right().max(o.right());
        let b = self.bottom().max(o.bottom());
        Rect::from_ltrb(l, t, r, b)
    }
    pub fn centered(cx: f32, cy: f32, w: f32, h: f32) -> Rect {
        Rect::new(cx - w * 0.5, cy - h * 0.5, w, h)
    }
    /// Square of side `s` centered in this rect.
    pub fn center_square(&self, s: f32) -> Rect {
        Rect::centered(self.cx(), self.cy(), s, s)
    }
}

// ---------------------------------------------------------------------------
// Colors (straight alpha, linear 0..1 floats as D2D expects)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Color {
    pub const fn rgba(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }
    /// 0xRRGGBB
    pub const fn hex(v: u32) -> Self {
        Self {
            r: ((v >> 16) & 0xFF) as f32 / 255.0,
            g: ((v >> 8) & 0xFF) as f32 / 255.0,
            b: (v & 0xFF) as f32 / 255.0,
            a: 1.0,
        }
    }
    pub const fn white(a: f32) -> Self {
        Self { r: 1.0, g: 1.0, b: 1.0, a }
    }
    pub const fn black(a: f32) -> Self {
        Self { r: 0.0, g: 0.0, b: 0.0, a }
    }
    pub fn alpha(self, a: f32) -> Self {
        Self { a: self.a * a, ..self }
    }
    pub fn with_a(self, a: f32) -> Self {
        Self { a, ..self }
    }
    pub fn lerp(self, o: Color, t: f32) -> Color {
        Color {
            r: self.r + (o.r - self.r) * t,
            g: self.g + (o.g - self.g) * t,
            b: self.b + (o.b - self.b) * t,
            a: self.a + (o.a - self.a) * t,
        }
    }
    /// Parse "#RRGGBB" / "#RRGGBBAA".
    pub fn parse(s: &str) -> Option<Color> {
        let s = s.trim().trim_start_matches('#');
        let v = u32::from_str_radix(s, 16).ok()?;
        match s.len() {
            6 => Some(Color::hex(v)),
            8 => Some(Color::hex(v >> 8).with_a((v & 0xFF) as f32 / 255.0)),
            _ => None,
        }
    }
    /// Make sure a (possibly dark) accent color stays readable on black.
    pub fn brighten_for_dark(self) -> Color {
        let lum = 0.2126 * self.r + 0.7152 * self.g + 0.0722 * self.b;
        if lum >= 0.45 {
            return self;
        }
        let max = self.r.max(self.g).max(self.b).max(0.001);
        // scale up so the brightest channel is near 1, then mix towards white a little
        let k = (0.95 / max).min(4.0);
        let c = Color::rgba((self.r * k).min(1.0), (self.g * k).min(1.0), (self.b * k).min(1.0), self.a);
        c.lerp(Color::white(self.a), 0.18)
    }
}

#[allow(dead_code)]
pub mod palette {
    use super::Color;
    pub const TEXT: Color = Color::white(0.96);
    pub const TEXT_DIM: Color = Color::white(0.58);
    pub const TEXT_FAINT: Color = Color::white(0.36);
    pub const CARD: Color = Color::white(0.065);
    pub const CARD_HOVER: Color = Color::white(0.11);
    pub const STROKE: Color = Color::white(0.08);
    pub const GREEN: Color = Color::hex(0x34C759);
    pub const ORANGE: Color = Color::hex(0xFF9F0A);
    pub const RED: Color = Color::hex(0xFF453A);
    pub const YELLOW: Color = Color::hex(0xFFD60A);
    pub const BLUE: Color = Color::hex(0x0A84FF);
    pub const TEAL: Color = Color::hex(0x64D2FF);
    pub const PURPLE: Color = Color::hex(0xBF5AF2);
    pub const PINK: Color = Color::hex(0xFF375F);
}

// ---------------------------------------------------------------------------
// Strings
// ---------------------------------------------------------------------------

/// UTF-16 with trailing NUL.
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// UTF-16 without trailing NUL.
pub fn utf16(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

pub fn from_wide(w: &[u16]) -> String {
    let end = w.iter().position(|&c| c == 0).unwrap_or(w.len());
    String::from_utf16_lossy(&w[..end])
}

/// Truncate to at most `max` chars (not bytes), appending an ellipsis.
pub fn truncate_chars(s: &str, max: usize) -> String {
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i >= max {
            out.push('…');
            return out;
        }
        out.push(ch);
    }
    out
}

/// Collapse whitespace runs (including newlines) into single spaces.
pub fn one_line(s: &str) -> String {
    let mut out = String::with_capacity(s.len().min(512));
    let mut last_space = false;
    for ch in s.chars() {
        if ch.is_whitespace() {
            if !last_space && !out.is_empty() {
                out.push(' ');
            }
            last_space = true;
        } else {
            out.push(ch);
            last_space = false;
        }
        if out.len() > 2000 {
            break;
        }
    }
    out.trim_end().to_string()
}

pub fn format_bytes(b: u64) -> String {
    const K: f64 = 1024.0;
    let b = b as f64;
    if b < K {
        format!("{b:.0} B")
    } else if b < K * K {
        format!("{:.0} KB", b / K)
    } else if b < K * K * K {
        format!("{:.1} MB", b / (K * K))
    } else {
        format!("{:.2} GB", b / (K * K * K))
    }
}

pub fn format_duration(secs: f64) -> String {
    let s = secs.max(0.0).round() as u64;
    let (h, m, s) = (s / 3600, (s / 60) % 60, s % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

// ---------------------------------------------------------------------------
// Thread-safe HWND handle
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SendHwnd(pub isize);
unsafe impl Send for SendHwnd {}
unsafe impl Sync for SendHwnd {}

impl SendHwnd {
    pub fn new(h: HWND) -> Self {
        Self(h.0 as isize)
    }
    pub fn hwnd(self) -> HWND {
        HWND(self.0 as *mut core::ffi::c_void)
    }
}

// ---------------------------------------------------------------------------
// Paths
// ---------------------------------------------------------------------------

/// %APPDATA%\DynamicNotch (created on demand).
pub fn app_dir() -> PathBuf {
    let base = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(|| std::env::temp_dir());
    let dir = base.join("DynamicNotch");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

// ---------------------------------------------------------------------------
// Logging (tiny file logger; no external crates)
// ---------------------------------------------------------------------------

static LOG: Mutex<Option<std::fs::File>> = Mutex::new(None);

pub fn log_init() {
    let path = app_dir().join("notch.log");
    // keep the log small: append, but start over once it passes 512 KB
    let big = std::fs::metadata(&path).map(|m| m.len() > 512 * 1024).unwrap_or(false);
    let f = std::fs::OpenOptions::new().create(true).append(!big).write(true).truncate(big).open(path);
    if let Ok(f) = f {
        *LOG.lock().unwrap() = Some(f);
    }
}

pub fn log_line(args: std::fmt::Arguments) {
    let line = format!("{args}\n");
    if cfg!(debug_assertions) {
        eprint!("{line}");
    }
    if let Ok(mut g) = LOG.lock() {
        if let Some(f) = g.as_mut() {
            let _ = f.write_all(line.as_bytes());
        }
    }
}

#[macro_export]
macro_rules! log {
    ($($t:tt)*) => { $crate::util::log_line(format_args!($($t)*)) };
}
