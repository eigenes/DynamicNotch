//! CPU-side images (premultiplied BGRA) + WIC decoding.
//!
//! Decoding happens on worker threads; the UI thread lazily uploads the
//! pixels to a D2D bitmap the first time an image is drawn.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use windows::core::Result;
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};

use crate::util::Color;

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone)]
pub struct ImageData {
    pub id: u64,
    pub width: u32,
    pub height: u32,
    /// Premultiplied BGRA, tightly packed.
    pub pixels: Arc<Vec<u8>>,
    /// Dominant vivid color, used to tint UI accents (e.g. visualizer bars).
    pub accent: Option<Color>,
}

impl std::fmt::Debug for ImageData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ImageData#{}({}x{})", self.id, self.width, self.height)
    }
}

impl ImageData {
    pub fn from_pbgra(width: u32, height: u32, pixels: Vec<u8>) -> Self {
        let accent = dominant_color(&pixels);
        Self { id: NEXT_ID.fetch_add(1, Ordering::Relaxed), width, height, pixels: Arc::new(pixels), accent }
    }
}

/// Decode any WIC-supported image (PNG, JPEG, BMP, GIF, WebP with codec...)
/// and downscale so the longest side is at most `max_side`.
/// COM must be initialised on the calling thread.
pub fn decode(bytes: &[u8], max_side: u32) -> Option<ImageData> {
    unsafe { decode_inner(bytes, max_side).ok() }
}

unsafe fn decode_inner(bytes: &[u8], max_side: u32) -> Result<ImageData> {
    let wic: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
    let stream = wic.CreateStream()?;
    stream.InitializeFromMemory(bytes)?;
    let decoder = wic.CreateDecoderFromStream(&stream, std::ptr::null(), WICDecodeMetadataCacheOnDemand)?;
    let frame = decoder.GetFrame(0)?;
    let source: IWICBitmapSource = windows::core::Interface::cast(&frame)?;
    from_source(&wic, &source, max_side)
}

pub unsafe fn from_source(wic: &IWICImagingFactory, source: &IWICBitmapSource, max_side: u32) -> Result<ImageData> {
    let (mut w, mut h) = (0u32, 0u32);
    source.GetSize(&mut w, &mut h)?;
    if w == 0 || h == 0 {
        return Err(windows::core::Error::from_hresult(windows::Win32::Foundation::E_FAIL));
    }
    let k = (max_side as f32 / w.max(h) as f32).min(1.0);
    let nw = ((w as f32 * k).round() as u32).max(1);
    let nh = ((h as f32 * k).round() as u32).max(1);
    let scaled: IWICBitmapSource = if nw != w || nh != h {
        let scaler = wic.CreateBitmapScaler()?;
        scaler.Initialize(source, nw, nh, WICBitmapInterpolationModeHighQualityCubic)?;
        windows::core::Interface::cast(&scaler)?
    } else {
        source.clone()
    };
    let conv = wic.CreateFormatConverter()?;
    conv.Initialize(
        &scaled,
        &GUID_WICPixelFormat32bppPBGRA,
        WICBitmapDitherTypeNone,
        None,
        0.0,
        WICBitmapPaletteTypeCustom,
    )?;
    let mut buf = vec![0u8; (nw * nh * 4) as usize];
    conv.CopyPixels(std::ptr::null(), nw * 4, &mut buf)?;
    Ok(ImageData::from_pbgra(nw, nh, buf))
}

/// Pick a vivid representative color: average of the most saturated pixels.
fn dominant_color(px: &[u8]) -> Option<Color> {
    let n = px.len() / 4;
    if n == 0 {
        return None;
    }
    let step = (n / 2048).max(1);
    let mut buckets: Vec<(f32, [f32; 3])> = Vec::with_capacity(n / step + 1);
    let mut i = 0;
    while i < n {
        let b = px[i * 4] as f32 / 255.0;
        let g = px[i * 4 + 1] as f32 / 255.0;
        let r = px[i * 4 + 2] as f32 / 255.0;
        let a = px[i * 4 + 3] as f32 / 255.0;
        if a > 0.5 {
            let (r, g, b) = (r / a, g / a, b / a);
            let max = r.max(g).max(b);
            let min = r.min(g).min(b);
            let sat = if max > 0.0 { (max - min) / max } else { 0.0 };
            // favour saturated, not-too-dark colors
            let score = sat * (0.35 + max);
            buckets.push((score, [r, g, b]));
        }
        i += step;
    }
    if buckets.is_empty() {
        return None;
    }
    buckets.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    let take = (buckets.len() / 8).max(1);
    let mut acc = [0.0f32; 3];
    for (_, c) in buckets.iter().take(take) {
        acc[0] += c[0];
        acc[1] += c[1];
        acc[2] += c[2];
    }
    let t = take as f32;
    let c = Color::rgba(acc[0] / t, acc[1] / t, acc[2] / t, 1.0);
    let max = c.r.max(c.g).max(c.b);
    let min = c.r.min(c.g).min(c.b);
    if max - min < 0.12 {
        // basically grayscale artwork → neutral white accent
        return Some(Color::white(0.9));
    }
    Some(c.brighten_for_dark())
}
