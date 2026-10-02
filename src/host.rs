//! Windows.UI.Composition hosting.
//!
//! Visual tree (bottom → top):
//!   root
//!   ├── backdrop  – HostBackdropBrush (real blur of what is behind the
//!   │               window), clipped to the notch body by a rounded-rect
//!   │               geometry that follows the animation.
//!   ├── content   – 2-buffer composition swap chain rendered with Direct2D
//!                   (fixed, small memory footprint; drawing surfaces pool
//!                   large atlas tiles and cost ~60 MB more on iGPUs).
//!   └── bars      – audio visualizer, animated entirely by the compositor
//!                   (zero CPU on our side while music plays).
//!

use std::mem::ManuallyDrop;
use windows::core::{Interface, Result, HSTRING};

use windows::Foundation::TimeSpan;
use windows::System::DispatcherQueueController;
use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::Graphics::Direct2D::Common::{D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_PIXEL_FORMAT};
use windows::Win32::Graphics::Direct2D::{
    ID2D1DeviceContext, D2D1_BITMAP_OPTIONS_CANNOT_DRAW, D2D1_BITMAP_OPTIONS_TARGET, D2D1_BITMAP_PROPERTIES1,
    D2D1_DEVICE_CONTEXT_OPTIONS_NONE,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_ALPHA_MODE_PREMULTIPLIED, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_UNKNOWN, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory2, IDXGIDevice1, IDXGIFactory2, IDXGISurface, IDXGISwapChain1, DXGI_CREATE_FACTORY_FLAGS,
    DXGI_PRESENT, DXGI_SCALING_STRETCH, DXGI_SWAP_CHAIN_DESC1, DXGI_SWAP_CHAIN_FLAG, DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
    DXGI_USAGE_RENDER_TARGET_OUTPUT,
};
use windows::Win32::System::WinRT::Composition::{ICompositorDesktopInterop, ICompositorInterop};
use windows::Win32::System::WinRT::{
    CreateDispatcherQueueController, DispatcherQueueOptions, DQTAT_COM_NONE, DQTYPE_THREAD_CURRENT,
};
use windows::UI::Composition::Desktop::DesktopWindowTarget;
use windows::UI::Composition::*;
use windows_numerics::{Vector2, Vector3};

use crate::gfx::Gfx;
use crate::util::{Color, Rect};

pub struct Host {
    _dq: DispatcherQueueController,
    pub compositor: Compositor,
    _target: DesktopWindowTarget,
    _root: ContainerVisual,
    backdrop: SpriteVisual,
    backdrop_geom: CompositionRoundedRectangleGeometry,
    backdrop_visible: bool,
    _content: SpriteVisual,
    sbrush: CompositionSurfaceBrush,
    swapchain: IDXGISwapChain1,
    dc: ID2D1DeviceContext,
    pub size: (i32, i32),
    pub bars: Bars,
}

fn v2(x: f32, y: f32) -> Vector2 {
    Vector2 { X: x, Y: y }
}

impl Host {
    pub fn new(hwnd: HWND, gfx: &Gfx, w: i32, h: i32) -> Result<Self> {
        unsafe {
            let dq = CreateDispatcherQueueController(DispatcherQueueOptions {
                dwSize: std::mem::size_of::<DispatcherQueueOptions>() as u32,
                threadType: DQTYPE_THREAD_CURRENT,
                apartmentType: DQTAT_COM_NONE,
            })?;
            let compositor = Compositor::new()?;
            let desktop: ICompositorDesktopInterop = compositor.cast()?;
            let target = desktop.CreateDesktopWindowTarget(hwnd, true)?;
            let root = compositor.CreateContainerVisual()?;
            root.SetRelativeSizeAdjustment(v2(1.0, 1.0))?;
            target.SetRoot(&root)?;

            // Blurred backdrop
            let backdrop = compositor.CreateSpriteVisual()?;
            let brush = compositor.CreateHostBackdropBrush()?;
            backdrop.SetBrush(&brush)?;
            backdrop.SetRelativeSizeAdjustment(v2(1.0, 1.0))?;
            let backdrop_geom = compositor.CreateRoundedRectangleGeometry()?;
            let clip = compositor.CreateGeometricClipWithGeometry(&backdrop_geom)?;
            backdrop.SetClip(&clip)?;
            backdrop.SetIsVisible(false)?;
            root.Children()?.InsertAtTop(&backdrop)?;

            // Direct2D content
            let swapchain = create_swapchain(gfx, w, h)?;
            let interop: ICompositorInterop = compositor.cast()?;
            let surface = interop.CreateCompositionSurfaceForSwapChain(&swapchain)?;
            let sbrush = compositor.CreateSurfaceBrushWithSurface(&surface)?;
            let dc = gfx.d2d_device.CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)?;
            sbrush.SetStretch(CompositionStretch::None)?;
            sbrush.SetHorizontalAlignmentRatio(0.0)?;
            sbrush.SetVerticalAlignmentRatio(0.0)?;
            let content = compositor.CreateSpriteVisual()?;
            content.SetBrush(&sbrush)?;
            content.SetRelativeSizeAdjustment(v2(1.0, 1.0))?;
            root.Children()?.InsertAtTop(&content)?;

            let bars = Bars::new(&compositor)?;
            root.Children()?.InsertAtTop(&bars.root)?;

            Ok(Self {
                _dq: dq,
                compositor,
                _target: target,
                _root: root,
                backdrop,
                backdrop_geom,
                backdrop_visible: false,
                _content: content,
                sbrush,
                swapchain,
                dc,
                size: (w, h),
                bars,
            })
        }
    }

    pub fn resize(&mut self, w: i32, h: i32) -> Result<()> {
        if (w, h) != self.size {
            unsafe {
                self.swapchain.ResizeBuffers(2, w as u32, h as u32, DXGI_FORMAT_UNKNOWN, DXGI_SWAP_CHAIN_FLAG(0))?;
            }
            self.size = (w, h);
        }
        Ok(())
    }

    /// Begin drawing into the back buffer. Returns the DC and the pixel
    /// offset of the drawable area (always 0,0 for a swap chain).
    pub fn begin_draw(&self) -> Result<(ID2D1DeviceContext, POINT)> {
        unsafe {
            let buf: IDXGISurface = self.swapchain.GetBuffer(0)?;
            let props = D2D1_BITMAP_PROPERTIES1 {
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                },
                dpiX: 96.0,
                dpiY: 96.0,
                bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET | D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
                colorContext: ManuallyDrop::new(None),
            };
            let target = self.dc.CreateBitmapFromDxgiSurface(&buf, Some(&props))?;
            self.dc.SetTarget(&target);
            self.dc.BeginDraw();
        }
        Ok((self.dc.clone(), POINT::default()))
    }

    pub fn end_draw(&self) -> Result<()> {
        unsafe {
            let r = self.dc.EndDraw(None, None);
            // drop the back buffer reference so ResizeBuffers keeps working
            self.dc.SetTarget(None);
            r?;
            self.swapchain.Present(0, DXGI_PRESENT(0)).ok()
        }
    }

    /// Replace the render context. A D2D device context keeps glyph caches
    /// and scratch buffers alive; dropping it when the notch goes idle hands
    /// that memory back (tens of MB on integrated GPUs).
    pub fn reset_dc(&mut self, gfx: &Gfx) {
        if let Ok(dc) = unsafe { gfx.d2d_device.CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE) } {
            self.dc = dc;
        }
    }

    /// After a device loss: rebuild the swap chain on the new device.
    pub fn set_rendering_device(&mut self, gfx: &Gfx) -> Result<()> {
        unsafe {
            let (w, h) = self.size;
            let swapchain = create_swapchain(gfx, w, h)?;
            let interop: ICompositorInterop = self.compositor.cast()?;
            let surface = interop.CreateCompositionSurfaceForSwapChain(&swapchain)?;
            self.sbrush.SetSurface(&surface)?;
            self.dc = gfx.d2d_device.CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)?;
            self.swapchain = swapchain;
        }
        Ok(())
    }

    /// Position the blur behind the notch body (pixel coordinates).
    /// The rounded rect extends above the window so only the bottom corners are round.
    pub fn set_backdrop(&mut self, body_px: Rect, radius_px: f32, opacity: f32) {
        let visible = opacity > 0.002;
        if visible != self.backdrop_visible {
            let _ = self.backdrop.SetIsVisible(visible);
            self.backdrop_visible = visible;
        }
        if !visible {
            return;
        }
        let r = radius_px.max(0.0);
        let _ = self.backdrop_geom.SetOffset(v2(body_px.x, body_px.y - r));
        let _ = self.backdrop_geom.SetSize(v2(body_px.w, body_px.h + r));
        let _ = self.backdrop_geom.SetCornerRadius(v2(r, r));
        let _ = self.backdrop.SetOpacity(opacity);
    }
}

unsafe fn create_swapchain(gfx: &Gfx, w: i32, h: i32) -> Result<IDXGISwapChain1> {
    let factory: IDXGIFactory2 = CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0))?;
    let desc = DXGI_SWAP_CHAIN_DESC1 {
        Width: w.max(1) as u32,
        Height: h.max(1) as u32,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        Stereo: false.into(),
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
        BufferCount: 2,
        Scaling: DXGI_SCALING_STRETCH,
        SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
        AlphaMode: DXGI_ALPHA_MODE_PREMULTIPLIED,
        Flags: 0,
    };
    let sc = factory.CreateSwapChainForComposition(&gfx.d3d, &desc, None)?;
    if let Ok(d) = gfx.d3d.cast::<IDXGIDevice1>() {
        let _ = d.SetMaximumFrameLatency(1);
    }
    Ok(sc)
}

// ---------------------------------------------------------------------------
// Visualizer bars
// ---------------------------------------------------------------------------

const BAR_COUNT: usize = 4;

pub struct Bars {
    compositor: Compositor,
    pub root: ContainerVisual,
    shape: ShapeVisual,
    geoms: Vec<CompositionRoundedRectangleGeometry>,
    brush: CompositionColorBrush,
    playing: bool,
    dims: (f32, f32),
    color: Color,
    visible: bool,
}

impl Bars {
    fn new(c: &Compositor) -> Result<Self> {
        let root = c.CreateContainerVisual()?;
        let shape = c.CreateShapeVisual()?;
        let brush = c.CreateColorBrushWithColor(windows::UI::Color { A: 255, R: 255, G: 255, B: 255 })?;
        let mut geoms = Vec::new();
        for _ in 0..BAR_COUNT {
            let g = c.CreateRoundedRectangleGeometry()?;
            let s = c.CreateSpriteShapeWithGeometry(&g)?;
            s.SetFillBrush(&brush)?;
            shape.Shapes()?.Append(&s)?;
            geoms.push(g);
        }
        root.Children()?.InsertAtTop(&shape)?;
        root.SetIsVisible(false)?;
        Ok(Self {
            compositor: c.clone(),
            root,
            shape,
            geoms,
            brush,
            playing: false,
            dims: (0.0, 0.0),
            color: Color::white(1.0),
            visible: false,
        })
    }

    pub fn hide(&mut self) {
        if self.visible {
            let _ = self.root.SetIsVisible(false);
            self.visible = false;
        }
    }

    /// Place the bars inside `r` (pixels) with the given opacity.
    pub fn update(&mut self, r: Rect, color: Color, opacity: f32, playing: bool) {
        if opacity <= 0.003 || r.w < 2.0 {
            self.hide();
            return;
        }
        if !self.visible {
            let _ = self.root.SetIsVisible(true);
            self.visible = true;
        }
        let _ = self.root.SetOffset(Vector3 { X: r.x, Y: r.y, Z: 0.0 });
        let _ = self.root.SetOpacity(opacity);
        if color != self.color {
            self.color = color;
            let to8 = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
            let _ =
                self.brush.SetColor(windows::UI::Color { A: 255, R: to8(color.r), G: to8(color.g), B: to8(color.b) });
        }
        let dims = ((r.w * 2.0).round() / 2.0, (r.h * 2.0).round() / 2.0);
        if dims != self.dims || playing != self.playing {
            self.dims = dims;
            self.playing = playing;
            let _ = self.shape.SetSize(Vector2 { X: dims.0, Y: dims.1 });
            let _ = self.rebuild();
        }
    }

    fn rebuild(&self) -> Result<()> {
        let (w, h) = self.dims;
        let n = BAR_COUNT as f32;
        let bw = (w / (n * 1.9)).max(1.0);
        let gap = (w - bw * n) / (n - 1.0);
        let min_h = bw.max(h * 0.18);
        // Pseudo-random but stable patterns per bar.
        const PATTERNS: [[f32; 6]; BAR_COUNT] = [
            [0.35, 0.90, 0.50, 1.00, 0.40, 0.75],
            [0.80, 0.40, 1.00, 0.55, 0.95, 0.45],
            [0.50, 1.00, 0.35, 0.85, 0.60, 0.95],
            [0.95, 0.55, 0.75, 0.40, 1.00, 0.60],
        ];
        const DURATIONS: [i64; BAR_COUNT] = [1150, 950, 1250, 1050];
        let size_name = HSTRING::from("Size");
        let off_name = HSTRING::from("Offset");
        for (i, g) in self.geoms.iter().enumerate() {
            let x = i as f32 * (bw + gap);
            g.SetCornerRadius(v2(bw * 0.5, bw * 0.5))?;
            let _ = g.StopAnimation(&size_name);
            let _ = g.StopAnimation(&off_name);
            if !self.playing {
                let bh = min_h;
                g.SetSize(v2(bw, bh))?;
                g.SetOffset(v2(x, (h - bh) * 0.5))?;
                continue;
            }
            let size_anim = self.compositor.CreateVector2KeyFrameAnimation()?;
            let off_anim = self.compositor.CreateVector2KeyFrameAnimation()?;
            let pat = PATTERNS[i];
            let steps = pat.len();
            for (k, &f) in pat.iter().chain(std::iter::once(&pat[0])).enumerate() {
                let t = k as f32 / steps as f32;
                let bh = (h * f).max(min_h);
                size_anim.InsertKeyFrame(t, v2(bw, bh))?;
                off_anim.InsertKeyFrame(t, v2(x, (h - bh) * 0.5))?;
            }
            let dur = TimeSpan { Duration: DURATIONS[i] * 10_000 };
            size_anim.SetDuration(dur)?;
            off_anim.SetDuration(dur)?;
            size_anim.SetIterationBehavior(AnimationIterationBehavior::Forever)?;
            off_anim.SetIterationBehavior(AnimationIterationBehavior::Forever)?;
            g.StartAnimation(&size_name, &size_anim)?;
            g.StartAnimation(&off_name, &off_anim)?;
        }
        Ok(())
    }
}
