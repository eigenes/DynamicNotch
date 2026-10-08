//! Rendering layer: Direct3D 11 device + Direct2D + DirectWrite + WIC.
//!
//! `Gfx` owns the long-lived device objects, `Res` caches device dependent
//! resources (brushes, text formats, bitmaps) across frames and `Painter`
//! is the immediate drawing API handed to the UI each frame.

pub mod icons;
pub mod image;

use std::collections::HashMap;
use std::mem::ManuallyDrop;

use windows::core::{Interface, Result, BOOL, PCWSTR};
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::Direct3D::*;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows_numerics::{Matrix3x2, Vector2};

pub use icons::Icon;
pub use image::ImageData;

use crate::util::{utf16, wide, Color, Rect};

// ---------------------------------------------------------------------------
// Devices
// ---------------------------------------------------------------------------

pub struct Gfx {
    pub d3d: ID3D11Device,
    pub d2d_factory: ID2D1Factory1,
    pub d2d_device: ID2D1Device,
    pub dwrite: IDWriteFactory,
    pub fonts: FontNames,
}

#[derive(Clone, Debug)]
pub struct FontNames {
    pub text: Vec<u16>,
    pub display: Vec<u16>,
    pub icons: Vec<u16>,
    pub mono: Vec<u16>,
}

impl Gfx {
    pub fn new() -> Result<Self> {
        unsafe {
            let d3d = create_d3d_device()?;
            let opts = D2D1_FACTORY_OPTIONS { debugLevel: D2D1_DEBUG_LEVEL_NONE };
            let d2d_factory: ID2D1Factory1 = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, Some(&opts))?;
            let dxgi: IDXGIDevice = d3d.cast()?;
            let d2d_device = d2d_factory.CreateDevice(&dxgi)?;
            let dwrite: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let fonts = pick_fonts(&dwrite);
            Ok(Self { d3d, d2d_factory, d2d_device, dwrite, fonts })
        }
    }

    /// Give driver scratch memory back once rendering goes idle. On laptops
    /// with integrated GPUs this memory is charged to the process.
    pub fn trim(&self) {
        unsafe {
            self.d2d_device.ClearResources(0);
            if let Ok(d) = self.d3d.cast::<windows::Win32::Graphics::Dxgi::IDXGIDevice3>() {
                d.Trim();
            }
        }
    }

    /// Recreate the GPU devices after a device-lost error.
    pub fn recreate_devices(&mut self) -> Result<()> {
        unsafe {
            let d3d = create_d3d_device()?;
            let dxgi: IDXGIDevice = d3d.cast()?;
            self.d2d_device = self.d2d_factory.CreateDevice(&dxgi)?;
            self.d3d = d3d;
        }
        Ok(())
    }
}

unsafe fn create_d3d_device() -> Result<ID3D11Device> {
    let levels = [
        D3D_FEATURE_LEVEL_11_1,
        D3D_FEATURE_LEVEL_11_0,
        D3D_FEATURE_LEVEL_10_1,
        D3D_FEATURE_LEVEL_10_0,
        D3D_FEATURE_LEVEL_9_3,
    ];
    let mut dev = None;
    let hr = D3D11CreateDevice(
        None,
        D3D_DRIVER_TYPE_HARDWARE,
        HMODULE::default(),
        D3D11_CREATE_DEVICE_BGRA_SUPPORT,
        Some(&levels),
        D3D11_SDK_VERSION,
        Some(&mut dev),
        None,
        None,
    );
    if hr.is_err() || dev.is_none() {
        // Fall back to the WARP software rasterizer (VMs, broken drivers).
        dev = None;
        D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_WARP,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            Some(&levels),
            D3D11_SDK_VERSION,
            Some(&mut dev),
            None,
            None,
        )?;
    }
    Ok(dev.unwrap())
}

fn pick_fonts(dwrite: &IDWriteFactory) -> FontNames {
    let exists = |name: &str| -> bool {
        unsafe {
            let mut coll = None;
            if dwrite.GetSystemFontCollection(&mut coll, false).is_err() {
                return false;
            }
            let Some(coll) = coll else { return false };
            let mut idx = 0u32;
            let mut found = BOOL(0);
            let w = wide(name);
            coll.FindFamilyName(PCWSTR(w.as_ptr()), &mut idx, &mut found).is_ok() && found.as_bool()
        }
    };
    let first = |cands: &[&str]| -> Vec<u16> {
        for c in cands {
            if exists(c) {
                return wide(c);
            }
        }
        wide(cands[cands.len() - 1])
    };
    FontNames {
        text: first(&["Segoe UI Variable Text", "Segoe UI"]),
        display: first(&["Segoe UI Variable Display", "Segoe UI"]),
        icons: first(&["Segoe Fluent Icons", "Segoe MDL2 Assets"]),
        mono: first(&["Cascadia Mono", "Consolas"]),
    }
}

// ---------------------------------------------------------------------------
// Text styles
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Font {
    Text,
    Display,
    Icons,
    Mono,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Align {
    Start,
    Center,
    End,
}

#[derive(Clone, Copy, Debug)]
pub struct TextStyle {
    pub size: f32,
    pub weight: u16,
    pub color: Color,
    pub font: Font,
    pub align: Align,
    pub valign: Align,
    pub wrap: bool,
}

impl TextStyle {
    pub fn new(size: f32, color: Color) -> Self {
        Self { size, weight: 400, color, font: Font::Text, align: Align::Start, valign: Align::Center, wrap: false }
    }
    pub fn weight(mut self, w: u16) -> Self {
        self.weight = w;
        self
    }
    pub fn semibold(self) -> Self {
        self.weight(600)
    }
    pub fn medium(self) -> Self {
        self.weight(500)
    }
    pub fn display(mut self) -> Self {
        self.font = Font::Display;
        self
    }
    pub fn mono(mut self) -> Self {
        self.font = Font::Mono;
        self
    }
    pub fn center(mut self) -> Self {
        self.align = Align::Center;
        self
    }
    pub fn right(mut self) -> Self {
        self.align = Align::End;
        self
    }
    pub fn top(mut self) -> Self {
        self.valign = Align::Start;
        self
    }
    pub fn wrap(mut self) -> Self {
        self.wrap = true;
        self
    }
    pub fn color(mut self, c: Color) -> Self {
        self.color = c;
        self
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct FmtKey {
    font: Font,
    size_x100: u32,
    weight: u16,
}

struct Fmt {
    format: IDWriteTextFormat,
    ellipsis: Option<IDWriteInlineObject>,
}

struct CachedBitmap {
    brush: ID2D1BitmapBrush1,
    w: u32,
    h: u32,
    last_used: u64,
}

// ---------------------------------------------------------------------------
// Resource cache (bound to the D2D device)
// ---------------------------------------------------------------------------

pub struct Res {
    dc: ID2D1DeviceContext,
    brush: ID2D1SolidColorBrush,
    formats: HashMap<FmtKey, Fmt>,
    bitmaps: HashMap<u64, CachedBitmap>,
    tnum: Option<IDWriteTypography>,
    shadow: Option<ID2D1Bitmap1>,
    /// Tessellated shapes kept on the GPU (see `Painter::fill_cached`).
    realizations: std::cell::RefCell<HashMap<RKey, (ID2D1GeometryRealization, u64)>>,
    frame: u64,
}

/// Cache key for a realized shape: kind + quantized dimensions.
pub type RKey = (u8, i32, i32, i32, i32);

/// Quantize to 1/4 DIP so nearly identical shapes share one realization.
pub fn q(v: f32) -> i32 {
    (v * 4.0).round() as i32
}

pub fn uq(v: i32) -> f32 {
    v as f32 * 0.25
}

impl Res {
    pub fn new(gfx: &Gfx) -> Result<Self> {
        unsafe {
            let dc = gfx.d2d_device.CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)?;
            let brush = dc.CreateSolidColorBrush(&d2c(Color::white(1.0)), None)?;
            let tnum = gfx.dwrite.CreateTypography().ok();
            if let Some(t) = &tnum {
                let _ = t.AddFontFeature(DWRITE_FONT_FEATURE {
                    nameTag: DWRITE_FONT_FEATURE_TAG_TABULAR_FIGURES,
                    parameter: 1,
                });
            }
            Ok(Self {
                dc,
                brush,
                formats: HashMap::new(),
                bitmaps: HashMap::new(),
                tnum,
                shadow: None,
                realizations: std::cell::RefCell::new(HashMap::new()),
                frame: 0,
            })
        }
    }

    /// Drop cached bitmaps that are not used right now (they are re-uploaded on demand).
    pub fn trim_bitmaps(&mut self) {
        let f = self.frame;
        self.bitmaps.retain(|_, b| f.saturating_sub(b.last_used) < 2);
    }

    /// Called once per frame; evicts bitmaps unused for a while.
    pub fn end_frame(&mut self) {
        self.frame += 1;
        let f = self.frame;
        if f.is_multiple_of(120) {
            self.bitmaps.retain(|_, b| f - b.last_used < 600);
            self.realizations.borrow_mut().retain(|_, r| f - r.1 < 600);
        }
    }

    fn format(&mut self, gfx: &Gfx, font: Font, size: f32, weight: u16) -> Option<&Fmt> {
        // degenerate sizes happen mid-animation; DirectWrite rejects them
        if !(1.0..2000.0).contains(&size) {
            return None;
        }
        let key = FmtKey { font, size_x100: (size * 100.0) as u32, weight };
        if let std::collections::hash_map::Entry::Vacant(e) = self.formats.entry(key) {
            let fmt = unsafe { Self::create_format(gfx, font, size, weight) }?;
            e.insert(fmt);
        }
        self.formats.get(&key)
    }

    unsafe fn create_format(gfx: &Gfx, font: Font, size: f32, weight: u16) -> Option<Fmt> {
        {
            let family = match font {
                Font::Text => &gfx.fonts.text,
                Font::Display => &gfx.fonts.display,
                Font::Icons => &gfx.fonts.icons,
                Font::Mono => &gfx.fonts.mono,
            };
            let format = gfx
                .dwrite
                .CreateTextFormat(
                    PCWSTR(family.as_ptr()),
                    None,
                    DWRITE_FONT_WEIGHT(weight as i32),
                    DWRITE_FONT_STYLE_NORMAL,
                    DWRITE_FONT_STRETCH_NORMAL,
                    size,
                    windows::core::w!("en-us"),
                )
                .ok()?;
            let ellipsis = gfx.dwrite.CreateEllipsisTrimmingSign(&format).ok();
            Some(Fmt { format, ellipsis })
        }
    }
}

pub fn d2c(c: Color) -> D2D1_COLOR_F {
    D2D1_COLOR_F { r: c.r, g: c.g, b: c.b, a: c.a }
}

fn rf(r: Rect) -> D2D_RECT_F {
    D2D_RECT_F { left: r.x, top: r.y, right: r.x + r.w, bottom: r.y + r.h }
}

fn v2(x: f32, y: f32) -> Vector2 {
    Vector2 { X: x, Y: y }
}

// ---------------------------------------------------------------------------
// Painter
// ---------------------------------------------------------------------------

pub struct TextLayout {
    pub layout: IDWriteTextLayout,
    pub width: f32,
    pub height: f32,
}

impl TextLayout {
    /// Caret slot right after UTF-16 index `pos - 1` (the end of the text
    /// when `pos` is its length): (x, line top, line height), layout-relative.
    pub fn caret(&self, pos: u32) -> (f32, f32, f32) {
        let (mut x, mut y) = (0.0, 0.0);
        let mut m = DWRITE_HIT_TEST_METRICS::default();
        unsafe {
            let _ = self.layout.HitTestTextPosition(pos.saturating_sub(1), pos > 0, &mut x, &mut y, &mut m);
        }
        (x, m.top, m.height)
    }
}

#[allow(dead_code)]
pub struct Painter<'a> {
    pub dc: ID2D1DeviceContext,
    pub gfx: &'a Gfx,
    res: &'a mut Res,
    /// Global opacity multiplier for everything drawn.
    pub alpha: f32,
    /// Device scale (pixels per logical unit) — used for pixel snapping.
    pub scale: f32,
    base: Matrix3x2,
    local: Matrix3x2,
    dc1: Option<ID2D1DeviceContext1>,
}

impl<'a> Painter<'a> {
    pub fn new(dc: ID2D1DeviceContext, gfx: &'a Gfx, res: &'a mut Res, scale: f32, ox: f32, oy: f32) -> Self {
        let base = Matrix3x2 { M11: scale, M12: 0.0, M21: 0.0, M22: scale, M31: ox, M32: oy };
        unsafe {
            dc.SetTransform(&base);
            dc.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
            dc.SetAntialiasMode(D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
        }
        let dc1 = dc.cast::<ID2D1DeviceContext1>().ok();
        Self { dc, gfx, res, alpha: 1.0, scale, base, local: Matrix3x2::identity(), dc1 }
    }

    #[allow(dead_code)]
    pub fn frame_number(&self) -> u64 {
        self.res.frame
    }

    pub fn clear(&self) {
        unsafe { self.dc.Clear(Some(&D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 0.0 })) }
    }

    /// Set an extra local transform: uniform scale `s` around (cx, cy) then translate (dx, dy).
    pub fn set_local(&mut self, s: f32, cx: f32, cy: f32, dx: f32, dy: f32) {
        self.local = Matrix3x2 { M11: s, M12: 0.0, M21: 0.0, M22: s, M31: cx - s * cx + dx, M32: cy - s * cy + dy };
        let m = self.local * self.base;
        unsafe { self.dc.SetTransform(&m) }
    }

    pub fn reset_local(&mut self) {
        self.local = Matrix3x2::identity();
        unsafe { self.dc.SetTransform(&self.base) }
    }

    fn brush(&self, c: Color) -> &ID2D1SolidColorBrush {
        unsafe { self.res.brush.SetColor(&d2c(c.alpha(self.alpha))) };
        &self.res.brush
    }

    pub fn push_clip(&self, r: Rect) {
        unsafe { self.dc.PushAxisAlignedClip(&rf(r), D2D1_ANTIALIAS_MODE_PER_PRIMITIVE) }
    }

    pub fn pop_clip(&self) {
        unsafe { self.dc.PopAxisAlignedClip() }
    }

    pub fn fill_rect(&self, r: Rect, c: Color) {
        if c.a * self.alpha <= 0.001 {
            return;
        }
        unsafe { self.dc.FillRectangle(&rf(r), self.brush(c)) }
    }

    pub fn fill_rounded(&self, r: Rect, radius: f32, c: Color) {
        if c.a * self.alpha <= 0.001 || r.w <= 0.0 || r.h <= 0.0 {
            return;
        }
        let rad = radius.min(r.w * 0.5).min(r.h * 0.5);
        if rad <= 0.01 {
            self.fill_rect(r, c);
            return;
        }
        let key = (1, q(r.w), q(r.h), q(rad), 0);
        self.fill_cached(key, r.x, r.y, c, |b| {
            b.rounded_rect(Rect::new(0.0, 0.0, uq(key.1), uq(key.2)), uq(key.3), true)
        });
    }

    /// Fill a shape that is built at the origin and drawn at (x, y).
    ///
    /// The tessellation is cached on the GPU as an ID2D1GeometryRealization
    /// keyed by `key`. Re-tessellating and streaming every rounded shape each
    /// frame makes some drivers (e.g. on ARM laptops) grow their dynamic
    /// upload heap by ~64 MB; realized geometry is uploaded once.
    pub fn fill_cached(&self, key: RKey, x: f32, y: f32, c: Color, build: impl FnOnce(&PathBuilder)) {
        if c.a * self.alpha <= 0.001 {
            return;
        }
        let at = Matrix3x2::translation(x, y) * self.local * self.base;
        let restore = self.local * self.base;
        let Some(dc1) = &self.dc1 else {
            // pre-8.1 fallback: plain path fill
            if let Some(g) = self.path(build) {
                unsafe {
                    self.dc.SetTransform(&at);
                    self.dc.FillGeometry(&g, self.brush(c), None);
                    self.dc.SetTransform(&restore);
                }
            }
            return;
        };
        let frame = self.res.frame;
        let mut cache = self.res.realizations.borrow_mut();
        if let std::collections::hash_map::Entry::Vacant(e) = cache.entry(key) {
            let Some(g) = self.path(build) else { return };
            let tol = D2D1_DEFAULT_FLATTENING_TOLERANCE / self.scale.max(1.0);
            let Ok(real) = (unsafe { dc1.CreateFilledGeometryRealization(&g, tol) }) else { return };
            e.insert((real, frame));
        }
        let entry = cache.get_mut(&key).unwrap();
        entry.1 = frame;
        unsafe {
            self.dc.SetTransform(&at);
            dc1.DrawGeometryRealization(&entry.0, self.brush(c));
            self.dc.SetTransform(&restore);
        }
    }

    // Outlines, lines and arcs are built as *filled* paths on purpose: on
    // some GPU drivers Direct2D's stroke pipeline allocates ~65 MB of scratch
    // memory on first use, while fills stay at a few MB.

    /// Rounded-rect outline of `width`, centered on the edge of `r`.
    pub fn stroke_rounded(&self, r: Rect, radius: f32, width: f32, c: Color) {
        if c.a * self.alpha <= 0.001 || r.w <= 0.0 || r.h <= 0.0 {
            return;
        }
        let rad = radius.min(r.w * 0.5).min(r.h * 0.5);
        let key = (2, q(r.w), q(r.h), q(rad), q(width));
        self.fill_cached(key, r.x, r.y, c, |b| {
            let (w, ht, rad, h) = (uq(key.1), uq(key.2), uq(key.3), uq(key.4) * 0.5);
            let local = Rect::new(0.0, 0.0, w, ht);
            b.rounded_rect(local.expand(h), rad + h, true);
            let inner = local.inset(h);
            if inner.w > 0.0 && inner.h > 0.0 {
                b.rounded_rect(inner, (rad - h).max(0.0), false);
            }
        });
    }

    pub fn fill_circle(&self, cx: f32, cy: f32, radius: f32, c: Color) {
        if c.a * self.alpha <= 0.001 {
            return;
        }
        let key = (5, q(radius), 0, 0, 0);
        self.fill_cached(key, cx, cy, c, |b| b.circle(0.0, 0.0, uq(key.1), true));
    }

    /// Ring of `width` centered on a circle of `radius`.
    pub fn stroke_circle(&self, cx: f32, cy: f32, radius: f32, width: f32, c: Color) {
        if c.a * self.alpha <= 0.001 {
            return;
        }
        let key = (3, q(radius), q(width), 0, 0);
        self.fill_cached(key, cx, cy, c, |b| {
            let (r, h) = (uq(key.1), uq(key.2) * 0.5);
            b.circle(0.0, 0.0, r + h, true);
            if r > h {
                b.circle(0.0, 0.0, r - h, false);
            }
        });
    }

    /// Line with round caps (a filled capsule).
    pub fn line(&self, x0: f32, y0: f32, x1: f32, y1: f32, width: f32, c: Color) {
        if c.a * self.alpha <= 0.001 {
            return;
        }
        let h = width * 0.5;
        let (dx, dy) = (x1 - x0, y1 - y0);
        let len = (dx * dx + dy * dy).sqrt();
        if len < 0.01 {
            self.fill_circle(x0, y0, h, c);
            return;
        }
        let key = (4, q(dx), q(dy), q(width), 0);
        self.fill_cached(key, x0, y0, c, |b| {
            let (dx, dy, h) = (uq(key.1), uq(key.2), uq(key.3) * 0.5);
            let len = (dx * dx + dy * dy).sqrt().max(0.001);
            let (nx, ny) = (-dy / len * h, dx / len * h);
            b.move_to(nx, ny);
            b.line_to(dx + nx, dy + ny);
            b.arc_to(dx - nx, dy - ny, h, false);
            b.line_to(-nx, -ny);
            b.arc_to(nx, ny, h, false);
            b.close();
        });
    }

    pub fn fill_geometry(&self, g: &ID2D1Geometry, c: Color) {
        unsafe { self.dc.FillGeometry(g, self.brush(c), None) }
    }

    /// Circular arc with round caps from `start` (radians, 0 = 12 o'clock,
    /// clockwise) spanning `sweep`.
    #[allow(clippy::too_many_arguments)]
    pub fn arc(&self, cx: f32, cy: f32, radius: f32, start: f32, sweep: f32, width: f32, c: Color) {
        if sweep.abs() < 0.001 || c.a * self.alpha <= 0.001 {
            return;
        }
        if sweep.abs() >= std::f32::consts::TAU - 0.001 {
            self.stroke_circle(cx, cy, radius, width, c);
            return;
        }
        let (start, sweep) = if sweep < 0.0 { (start + sweep, -sweep) } else { (start, sweep) };
        let end = start + sweep;
        let h = width * 0.5;
        let pt = |a: f32, r: f32| (cx + r * a.sin(), cy - r * a.cos());
        let large = sweep > std::f32::consts::PI;
        let ri = (radius - h).max(0.0);
        if let Some(g) = self.path(|b| {
            let (x, y) = pt(start, radius + h);
            b.move_to(x, y);
            let (x, y) = pt(end, radius + h);
            b.arc_to_ex(x, y, radius + h, true, large);
            let (x, y) = pt(end, ri);
            b.arc_to(x, y, h, true); // end cap
            let (x, y) = pt(start, ri);
            b.arc_to_ex(x, y, ri, false, large);
            let (x, y) = pt(start, radius + h);
            b.arc_to(x, y, h, true); // start cap
            b.close();
        }) {
            self.fill_geometry(&g, c);
        }
    }

    /// Path builder using the shared factory.
    pub fn path(&self, build: impl FnOnce(&PathBuilder)) -> Option<ID2D1Geometry> {
        unsafe {
            let path = self.gfx.d2d_factory.CreatePathGeometry().ok()?;
            let sink = path.Open().ok()?;
            sink.SetFillMode(D2D1_FILL_MODE_WINDING);
            let b = PathBuilder { sink: sink.clone(), open: std::cell::Cell::new(false) };
            build(&b);
            b.finish();
            sink.Close().ok()?;
            path.cast().ok()
        }
    }

    // -- text ---------------------------------------------------------------

    pub fn layout(&mut self, s: &str, style: &TextStyle, max_w: f32, max_h: f32) -> Option<TextLayout> {
        let gfx = self.gfx;
        let fmt = self.res.format(gfx, style.font, style.size, style.weight)?;
        let text = utf16(s);
        if !(max_w.is_finite() && max_h.is_finite()) {
            return None;
        }
        unsafe {
            let layout = gfx.dwrite.CreateTextLayout(&text, &fmt.format, max_w.max(1.0), max_h.max(1.0)).ok()?;
            let _ = layout.SetTextAlignment(match style.align {
                Align::Start => DWRITE_TEXT_ALIGNMENT_LEADING,
                Align::Center => DWRITE_TEXT_ALIGNMENT_CENTER,
                Align::End => DWRITE_TEXT_ALIGNMENT_TRAILING,
            });
            let _ = layout.SetParagraphAlignment(match style.valign {
                Align::Start => DWRITE_PARAGRAPH_ALIGNMENT_NEAR,
                Align::Center => DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
                Align::End => DWRITE_PARAGRAPH_ALIGNMENT_FAR,
            });
            if !style.wrap {
                let _ = layout.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP);
                if let Some(e) = &fmt.ellipsis {
                    let t = DWRITE_TRIMMING {
                        granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER,
                        delimiter: 0,
                        delimiterCount: 0,
                    };
                    let _ = layout.SetTrimming(&t, e);
                }
            } else {
                let _ = layout.SetWordWrapping(DWRITE_WORD_WRAPPING_WRAP);
            }
            if matches!(style.font, Font::Display | Font::Mono) {
                if let Some(t) = &self.res.tnum {
                    let _ = layout.SetTypography(t, DWRITE_TEXT_RANGE { startPosition: 0, length: text.len() as u32 });
                }
            }
            let mut m = DWRITE_TEXT_METRICS::default();
            let _ = layout.GetMetrics(&mut m);
            Some(TextLayout { layout, width: m.widthIncludingTrailingWhitespace, height: m.height })
        }
    }

    pub fn draw_layout(&self, l: &TextLayout, x: f32, y: f32, c: Color) {
        unsafe { self.dc.DrawTextLayout(v2(x, y), &l.layout, self.brush(c), D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT) }
    }

    /// Draw text inside `r` (single line with ellipsis unless style.wrap).
    pub fn text(&mut self, s: &str, r: Rect, style: &TextStyle) {
        if s.is_empty() || style.color.a * self.alpha <= 0.001 {
            return;
        }
        if let Some(l) = self.layout(s, style, r.w, r.h) {
            self.draw_layout(&l, r.x, r.y, style.color);
        }
    }

    /// Text in `style.color` with a soft band of `hi` centred on x = `cx`,
    /// fading out over `half` on each side (the loading-label shimmer).
    pub fn text_shimmer(&mut self, s: &str, r: Rect, style: &TextStyle, hi: Color, cx: f32, half: f32) {
        if s.is_empty() || half <= 0.0 {
            return;
        }
        let Some(l) = self.layout(s, style, r.w, r.h) else { return };
        let base = d2c(style.color.alpha(self.alpha));
        let stops = [
            D2D1_GRADIENT_STOP { position: 0.0, color: base },
            D2D1_GRADIENT_STOP { position: 0.5, color: d2c(hi.alpha(self.alpha)) },
            D2D1_GRADIENT_STOP { position: 1.0, color: base },
        ];
        let props =
            D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES { startPoint: v2(cx - half, 0.0), endPoint: v2(cx + half, 0.0) };
        unsafe {
            let Ok(coll) = ID2D1RenderTarget::CreateGradientStopCollection(
                &self.dc,
                &stops,
                D2D1_GAMMA_2_2,
                D2D1_EXTEND_MODE_CLAMP,
            ) else {
                return;
            };
            let Ok(brush) = self.dc.CreateLinearGradientBrush(&props, None, &coll) else { return };
            self.dc.DrawTextLayout(v2(r.x, r.y), &l.layout, &brush, D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT);
        }
    }

    /// Width/height of a single line of text.
    pub fn measure(&mut self, s: &str, style: &TextStyle) -> (f32, f32) {
        let st = TextStyle { wrap: false, ..*style };
        match self.layout(s, &st, 10_000.0, 1_000.0) {
            Some(l) => (l.width, l.height),
            None => (0.0, 0.0),
        }
    }

    /// Height of wrapped text in the given width.
    pub fn measure_wrapped(&mut self, s: &str, style: &TextStyle, w: f32) -> f32 {
        let st = TextStyle { wrap: true, valign: Align::Start, ..*style };
        self.layout(s, &st, w, 100_000.0).map(|l| l.height).unwrap_or(0.0)
    }

    /// Draw a glyph from the system icon font, centered in `r`.
    pub fn glyph(&mut self, ch: char, r: Rect, size: f32, c: Color) {
        let st = TextStyle {
            size,
            weight: 400,
            color: c,
            font: Font::Icons,
            align: Align::Center,
            valign: Align::Center,
            wrap: false,
        };
        let mut buf = [0u8; 4];
        let s = ch.encode_utf8(&mut buf);
        if let Some(l) = self.layout(s, &st, r.w.max(size * 2.0), r.h.max(size * 2.0)) {
            let x = r.cx() - r.w.max(size * 2.0) * 0.5;
            let y = r.cy() - r.h.max(size * 2.0) * 0.5;
            self.draw_layout(&l, x, y, c);
        }
    }

    pub fn icon(&mut self, icon: Icon, r: Rect, c: Color) {
        icons::draw(self, icon, r, c);
    }

    // -- shadow -------------------------------------------------------------

    /// Soft outer shadow around a rounded rect, drawn as a 9-slice of one
    /// small precomputed bitmap (cheap, and no per-size geometry caches).
    pub fn shadow(&mut self, r: Rect, radius: f32, opacity: f32) {
        if opacity * self.alpha <= 0.002 || r.w <= 1.0 || r.h <= 1.0 {
            return;
        }
        if self.res.shadow.is_none() {
            self.res.shadow = make_shadow_bitmap(&self.res.dc);
        }
        let Some(bmp) = &self.res.shadow else { return };
        let src_corner = (SHADOW_R + SHADOW_B) * SHADOW_SCALE;
        let size = 2.0 * src_corner + 2.0;
        // destination corner scales with the current corner radius
        let dst_corner = (radius.max(1.0) + SHADOW_B).min(r.w * 0.5 + SHADOW_B).min(r.h * 0.5 + SHADOW_B);
        let outer = r.expand(SHADOW_B);
        let xs_d = [outer.x, outer.x + dst_corner, outer.right() - dst_corner, outer.right()];
        let ys_d = [outer.y, outer.y + dst_corner, outer.bottom() - dst_corner, outer.bottom()];
        let xs_s = [0.0, src_corner, size - src_corner, size];
        for i in 0..3 {
            for j in 0..3 {
                let d = D2D_RECT_F { left: xs_d[i], top: ys_d[j], right: xs_d[i + 1], bottom: ys_d[j + 1] };
                if d.right <= d.left || d.bottom <= d.top {
                    continue;
                }
                let s = D2D_RECT_F { left: xs_s[i], top: xs_s[j], right: xs_s[i + 1], bottom: xs_s[j + 1] };
                unsafe {
                    self.dc.DrawBitmap(
                        bmp,
                        Some(&d),
                        opacity * self.alpha,
                        D2D1_INTERPOLATION_MODE_LINEAR,
                        Some(&s),
                        None,
                    );
                }
            }
        }
    }

    // -- images -------------------------------------------------------------

    /// Draw an image "aspect fill" into `r` with rounded corners.
    pub fn image(&mut self, img: &ImageData, r: Rect, radius: f32, opacity: f32) {
        if r.w <= 0.5 || r.h <= 0.5 {
            return;
        }
        let frame = self.res.frame;
        if !self.res.bitmaps.contains_key(&img.id) {
            let Some(cb) = self.create_bitmap(img) else { return };
            self.res.bitmaps.insert(img.id, cb);
        }
        let cb = self.res.bitmaps.get_mut(&img.id).unwrap();
        cb.last_used = frame;
        let (iw, ih) = (cb.w as f32, cb.h as f32);
        // cover: scale so the image fills r, centered
        let s = (r.w / iw).max(r.h / ih);
        let ox = r.x + (r.w - iw * s) * 0.5;
        let oy = r.y + (r.h - ih * s) * 0.5;
        let m = Matrix3x2 { M11: s, M12: 0.0, M21: 0.0, M22: s, M31: ox, M32: oy };
        unsafe {
            cb.brush.SetTransform(&m);
            cb.brush.SetOpacity(opacity * self.alpha);
            let rad = radius.min(r.w * 0.5).min(r.h * 0.5);
            let rr = D2D1_ROUNDED_RECT { rect: rf(r), radiusX: rad, radiusY: rad };
            self.dc.FillRoundedRectangle(&rr, &cb.brush);
        }
    }

    fn create_bitmap(&self, img: &ImageData) -> Option<CachedBitmap> {
        unsafe {
            let props = D2D1_BITMAP_PROPERTIES1 {
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                },
                dpiX: 96.0,
                dpiY: 96.0,
                bitmapOptions: D2D1_BITMAP_OPTIONS_NONE,
                colorContext: ManuallyDrop::new(None),
            };
            let bmp = self
                .res
                .dc
                .CreateBitmap(
                    D2D_SIZE_U { width: img.width, height: img.height },
                    Some(img.pixels.as_ptr() as *const _),
                    img.width * 4,
                    &props,
                )
                .ok()?;
            let bprops = D2D1_BITMAP_BRUSH_PROPERTIES1 {
                extendModeX: D2D1_EXTEND_MODE_CLAMP,
                extendModeY: D2D1_EXTEND_MODE_CLAMP,
                interpolationMode: D2D1_INTERPOLATION_MODE_HIGH_QUALITY_CUBIC,
            };
            let brush = self.res.dc.CreateBitmapBrush(&bmp, Some(&bprops), None).ok()?;
            Some(CachedBitmap { brush, w: img.width, h: img.height, last_used: self.res.frame })
        }
    }
}

const SHADOW_R: f32 = 34.0; // corner radius baked into the bitmap (DIPs)
const SHADOW_B: f32 = 22.0; // blur extent (DIPs)
const SHADOW_SCALE: f32 = 2.0; // bitmap pixels per DIP

/// Black gaussian falloff *outside* a rounded rect; transparent inside, so
/// it never darkens translucent glass drawn on top.
fn make_shadow_bitmap(dc: &ID2D1DeviceContext) -> Option<ID2D1Bitmap1> {
    let corner = (SHADOW_R + SHADOW_B) * SHADOW_SCALE;
    let n = (2.0 * corner + 2.0) as usize;
    let c = n as f32 * 0.5;
    let half = c - SHADOW_B * SHADOW_SCALE; // half-extent of the shape
    let r = SHADOW_R * SHADOW_SCALE;
    let sigma = SHADOW_B * SHADOW_SCALE / 2.6;
    let mut px = vec![0u8; n * n * 4];
    for y in 0..n {
        for x in 0..n {
            let qx = ((x as f32 + 0.5 - c).abs() - (half - r)).max(0.0);
            let qy = ((y as f32 + 0.5 - c).abs() - (half - r)).max(0.0);
            let d = (qx * qx + qy * qy).sqrt() - r;
            let a = if d <= 0.0 { 0.0 } else { (-0.5 * (d / sigma).powi(2)).exp() * (d / 1.5).min(1.0) };
            px[(y * n + x) * 4 + 3] = (a * 255.0).round() as u8; // premultiplied black
        }
    }
    let props = D2D1_BITMAP_PROPERTIES1 {
        pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
        dpiX: 96.0,
        dpiY: 96.0,
        bitmapOptions: D2D1_BITMAP_OPTIONS_NONE,
        colorContext: ManuallyDrop::new(None),
    };
    unsafe {
        dc.CreateBitmap(
            D2D_SIZE_U { width: n as u32, height: n as u32 },
            Some(px.as_ptr() as *const _),
            (n * 4) as u32,
            &props,
        )
        .ok()
    }
}

/// Helper to build path geometries without exposing the sink everywhere.
pub struct PathBuilder {
    sink: ID2D1GeometrySink,
    open: std::cell::Cell<bool>,
}

impl PathBuilder {
    pub fn move_to(&self, x: f32, y: f32) {
        unsafe {
            if self.open.get() {
                self.sink.EndFigure(D2D1_FIGURE_END_CLOSED);
            }
            self.sink.BeginFigure(v2(x, y), D2D1_FIGURE_BEGIN_FILLED);
        }
        self.open.set(true);
    }
    pub fn line_to(&self, x: f32, y: f32) {
        unsafe { self.sink.AddLine(v2(x, y)) }
    }
    /// Circular arc to (x, y).
    pub fn arc_to(&self, x: f32, y: f32, radius: f32, clockwise: bool) {
        unsafe {
            self.sink.AddArc(&D2D1_ARC_SEGMENT {
                point: v2(x, y),
                size: D2D_SIZE_F { width: radius, height: radius },
                rotationAngle: 0.0,
                sweepDirection: if clockwise {
                    D2D1_SWEEP_DIRECTION_CLOCKWISE
                } else {
                    D2D1_SWEEP_DIRECTION_COUNTER_CLOCKWISE
                },
                arcSize: D2D1_ARC_SIZE_SMALL,
            })
        }
    }
    pub fn arc_to_ex(&self, x: f32, y: f32, radius: f32, clockwise: bool, large: bool) {
        unsafe {
            self.sink.AddArc(&D2D1_ARC_SEGMENT {
                point: v2(x, y),
                size: D2D_SIZE_F { width: radius, height: radius },
                rotationAngle: 0.0,
                sweepDirection: if clockwise {
                    D2D1_SWEEP_DIRECTION_CLOCKWISE
                } else {
                    D2D1_SWEEP_DIRECTION_COUNTER_CLOCKWISE
                },
                arcSize: if large { D2D1_ARC_SIZE_LARGE } else { D2D1_ARC_SIZE_SMALL },
            })
        }
    }

    /// Closed rounded-rect figure. Opposite windings punch holes (winding fill).
    pub fn rounded_rect(&self, r: Rect, radius: f32, clockwise: bool) {
        let rad = radius.min(r.w * 0.5).min(r.h * 0.5).max(0.0);
        let (l, t, rt, b) = (r.x, r.y, r.right(), r.bottom());
        if clockwise {
            self.move_to(l + rad, t);
            self.line_to(rt - rad, t);
            self.arc_to(rt, t + rad, rad, true);
            self.line_to(rt, b - rad);
            self.arc_to(rt - rad, b, rad, true);
            self.line_to(l + rad, b);
            self.arc_to(l, b - rad, rad, true);
            self.line_to(l, t + rad);
            self.arc_to(l + rad, t, rad, true);
        } else {
            self.move_to(l + rad, t);
            self.arc_to(l, t + rad, rad, false);
            self.line_to(l, b - rad);
            self.arc_to(l + rad, b, rad, false);
            self.line_to(rt - rad, b);
            self.arc_to(rt, b - rad, rad, false);
            self.line_to(rt, t + rad);
            self.arc_to(rt - rad, t, rad, false);
            self.line_to(l + rad, t);
        }
        self.close();
    }

    pub fn circle(&self, cx: f32, cy: f32, r: f32, clockwise: bool) {
        self.move_to(cx, cy - r);
        self.arc_to(cx, cy + r, r, clockwise);
        self.arc_to(cx, cy - r, r, clockwise);
        self.close();
    }

    /// Closed polygon with corners rounded by `radius`.
    pub fn rounded_poly(&self, pts: &[(f32, f32)], radius: f32) {
        let n = pts.len();
        if n < 3 {
            return;
        }
        let corner = |i: usize| {
            let (x, y) = pts[i];
            let (px, py) = pts[(i + n - 1) % n];
            let (nx, ny) = pts[(i + 1) % n];
            let lp = ((px - x).powi(2) + (py - y).powi(2)).sqrt().max(0.001);
            let ln = ((nx - x).powi(2) + (ny - y).powi(2)).sqrt().max(0.001);
            let r = radius.min(lp * 0.5).min(ln * 0.5);
            ((x + (px - x) / lp * r, y + (py - y) / lp * r), (x + (nx - x) / ln * r, y + (ny - y) / ln * r))
        };
        let (_, out0) = corner(0);
        self.move_to(out0.0, out0.1);
        for i in (1..n).chain(std::iter::once(0)) {
            let (inp, out) = corner(i);
            self.line_to(inp.0, inp.1);
            self.quad_to(pts[i].0, pts[i].1, out.0, out.1);
        }
        self.close();
    }

    pub fn quad_to(&self, cx: f32, cy: f32, x: f32, y: f32) {
        unsafe { self.sink.AddQuadraticBezier(&D2D1_QUADRATIC_BEZIER_SEGMENT { point1: v2(cx, cy), point2: v2(x, y) }) }
    }
    pub fn close(&self) {
        if self.open.get() {
            unsafe { self.sink.EndFigure(D2D1_FIGURE_END_CLOSED) };
            self.open.set(false);
        }
    }
    fn finish(&self) {
        self.close();
    }
}

/// Notch silhouette: flush with the top edge, concave "ears" where it meets
/// the screen edge and rounded bottom corners.
pub fn notch_path(p: &Painter, body: Rect, bottom_radius: f32, ear: f32) -> Option<ID2D1Geometry> {
    let x0 = body.x;
    let x1 = body.right();
    let h = body.h.max(1.0);
    let r = bottom_radius.min(body.w * 0.5).min(h - ear.min(h * 0.5)).max(0.0);
    let e = ear.min(h * 0.5).max(0.0);
    p.path(|b| {
        b.move_to(x0 - e, 0.0);
        if e > 0.0 {
            b.arc_to(x0, e, e, true);
        }
        b.line_to(x0, h - r);
        if r > 0.0 {
            b.arc_to(x0 + r, h, r, false);
        }
        b.line_to(x1 - r, h);
        if r > 0.0 {
            b.arc_to(x1, h - r, r, false);
        }
        b.line_to(x1, e);
        if e > 0.0 {
            b.arc_to(x1 + e, 0.0, e, true);
        }
        b.close();
    })
}
