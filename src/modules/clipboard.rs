//! Recent clipboard items. Uses AddClipboardFormatListener (event driven).
//! History lives in memory only and content flagged by password managers
//! ("ExcludeClipboardContentFromMonitorProcessing") is never read.

use std::any::Any;
use std::time::{Duration, Instant};

use windows::core::w;
use windows::Win32::Foundation::HWND;
use windows::Win32::System::DataExchange::*;
use windows::Win32::System::Ole::{CF_DIB, CF_DIBV5, CF_HDROP, CF_UNICODETEXT};
use windows::Win32::UI::Shell::{DragQueryFileW, DROPFILES, HDROP};

use super::{CardSize, Cx, Module, ModuleId, Peek, SystemEvent};
use crate::gfx::{image, Icon, ImageData, TextStyle};
use crate::sys::clip::{self, with_global};
use crate::ui::{ButtonStyle, Ui};
use crate::util::{one_line, palette, truncate_chars, Rect, SendHwnd};

const ID: ModuleId = "clipboard";
const ROW_H: f32 = 36.0;
const MAX_ROWS: usize = 6;

#[derive(Clone, Debug, PartialEq)]
enum Kind {
    Text(String),
    Files(Vec<String>),
    Image { w: u32, h: u32 },
}

#[derive(Clone, Debug)]
struct Item {
    id: u64,
    kind: Kind,
    thumb: Option<ImageData>,
    at: Instant,
}

impl Item {
    fn summary(&self) -> String {
        match &self.kind {
            Kind::Text(t) => truncate_chars(&one_line(t), 160),
            Kind::Files(f) if f.len() == 1 => file_name(&f[0]),
            Kind::Files(f) => format!("{} and {} more", file_name(&f[0]), f.len() - 1),
            Kind::Image { w, h } => format!("Image  {w} × {h}"),
        }
    }
    fn icon(&self) -> Icon {
        match &self.kind {
            Kind::Text(t) if t.trim_start().starts_with("http") => Icon::Link,
            Kind::Text(_) => Icon::Clipboard,
            Kind::Files(f) if f.len() == 1 => Icon::File,
            Kind::Files(_) => Icon::Folder,
            Kind::Image { .. } => Icon::Image,
        }
    }
}

struct Thumb(u64, Option<ImageData>);

pub struct Clipboard {
    items: Vec<Item>,
    next_id: u64,
    max: usize,
    hwnd: SendHwnd,
    self_write: bool,
}

impl Clipboard {
    pub fn new() -> Self {
        Self { items: Vec::new(), next_id: 1, max: 8, hwnd: SendHwnd(0), self_write: false }
    }

    fn push(&mut self, kind: Kind) -> u64 {
        if let Some(pos) = self.items.iter().position(|i| i.kind == kind) {
            let mut it = self.items.remove(pos);
            it.at = Instant::now();
            let id = it.id;
            self.items.insert(0, it);
            return id;
        }
        let id = self.next_id;
        self.next_id += 1;
        self.items.insert(0, Item { id, kind, thumb: None, at: Instant::now() });
        self.items.truncate(self.max);
        id
    }

    fn copy_back(&mut self, idx: usize, cx_peek: &mut Vec<Peek>) {
        let Some(item) = self.items.get(idx).cloned() else { return };
        let ok = match &item.kind {
            Kind::Text(t) => clip::set_text(self.hwnd.hwnd(), t),
            Kind::Files(f) => set_files(self.hwnd.hwnd(), f),
            Kind::Image { .. } => false,
        };
        if ok {
            self.self_write = true;
            let it = self.items.remove(idx);
            self.items.insert(0, Item { at: Instant::now(), ..it });
            cx_peek.push(
                Peek::new(Icon::Check, palette::GREEN, "Copied to clipboard", item.summary())
                    .key("clipboard")
                    .duration_ms(1400),
            );
        }
    }
}

impl Module for Clipboard {
    fn id(&self) -> ModuleId {
        ID
    }
    fn title(&self) -> &str {
        "Clipboard"
    }
    fn icon(&self) -> Icon {
        Icon::Clipboard
    }

    fn start(&mut self, cx: &mut Cx) {
        self.hwnd = cx.hwnd;
        self.max = cx.cfg.clipboard.history;
    }

    fn stop(&mut self) {
        self.items.clear();
    }

    fn on_message(&mut self, msg: Box<dyn Any + Send>, cx: &mut Cx) {
        if let Ok(t) = msg.downcast::<Thumb>() {
            if let Some(it) = self.items.iter_mut().find(|i| i.id == t.0) {
                it.thumb = t.1;
                cx.fx.redraw = true;
            }
        }
    }

    fn on_system(&mut self, ev: &SystemEvent, cx: &mut Cx) {
        match ev {
            SystemEvent::ConfigReloaded => {
                self.max = cx.cfg.clipboard.history;
                self.items.truncate(self.max);
            }
            SystemEvent::ClipboardChanged => {
                let owner = unsafe { GetClipboardOwner() }.ok();
                if self.self_write || owner == Some(self.hwnd.hwnd()) {
                    self.self_write = false;
                    return;
                }
                let Some(read) = read_clipboard(self.hwnd.hwnd()) else { return };
                let (kind, dib) = read;
                let id = self.push(kind);
                if let Some(bytes) = dib {
                    let bus = cx.bus.clone();
                    let _ = std::thread::Builder::new().name("clip-thumb".into()).spawn(move || {
                        unsafe {
                            let _ = windows::Win32::System::Com::CoInitializeEx(
                                None,
                                windows::Win32::System::Com::COINIT_MULTITHREADED,
                            );
                        }
                        let img = image::decode(&bytes, 160);
                        bus.to_module(ID, Thumb(id, img));
                    });
                }
                if cx.cfg.clipboard.flash_on_copy {
                    if let Some(it) = self.items.first() {
                        cx.fx.peek(
                            Peek::new(it.icon(), palette::BLUE, "Copied", it.summary())
                                .key("clipboard")
                                .duration_ms(1400)
                                .page(ID),
                        );
                    }
                }
                cx.fx.redraw = true;
            }
            _ => {}
        }
    }

    fn card(&self) -> Option<CardSize> {
        (!self.items.is_empty()).then_some(CardSize::Small)
    }

    fn draw_card(&mut self, ui: &mut Ui, r: Rect) {
        if ui.card(r, true) {
            ui.fx.open_page = Some(ID);
        }
        let Some(it) = self.items.first() else { return };
        let inner = r.inset_xy(12.0, 0.0);
        let is = 18.0;
        match &it.thumb {
            Some(t) => ui.p.image(t, Rect::new(inner.x, r.cy() - 11.0, 22.0, 22.0), 5.0, 1.0),
            None => ui.p.icon(it.icon(), Rect::new(inner.x, r.cy() - is * 0.5, is, is), palette::BLUE),
        }
        let tx = inner.x + 30.0;
        ui.p.text(&it.summary(), Rect::new(tx, r.y, inner.right() - tx, r.h), &TextStyle::new(12.5, palette::TEXT));
    }

    fn has_page(&self) -> bool {
        true
    }

    fn page_height(&self, _w: f32) -> f32 {
        if self.items.is_empty() {
            110.0
        } else {
            28.0 + self.items.len().min(MAX_ROWS) as f32 * (ROW_H + 4.0)
        }
    }

    fn draw_page(&mut self, ui: &mut Ui, r: Rect) {
        let (head, body) = r.split_top(24.0, 4.0);
        ui.caption("CLIPBOARD HISTORY", Rect::new(head.x, head.y, 200.0, head.h));
        if self.items.is_empty() {
            ui.empty_state(body, Icon::Clipboard, "Copy something and it shows up here");
            return;
        }
        let clear = Rect::new(head.right() - 24.0, head.y, 24.0, 24.0);
        if ui.icon_button(clear, Icon::Trash, ButtonStyle::default().scale(0.55).fg(palette::TEXT_DIM)) {
            self.items.clear();
            return;
        }
        let mut peeks = Vec::new();
        let mut copy_idx = None;
        for (i, it) in self.items.iter().take(MAX_ROWS).enumerate() {
            let row = Rect::new(body.x, body.y + i as f32 * (ROW_H + 4.0), body.w, ROW_H);
            let copyable = !matches!(it.kind, Kind::Image { .. });
            let hover = copyable && ui.hovered(row);
            ui.p.fill_rounded(row, 10.0, if hover { palette::CARD_HOVER } else { palette::CARD });
            let ir = Rect::new(row.x + 10.0, row.cy() - 10.0, 20.0, 20.0);
            match &it.thumb {
                Some(t) => ui.p.image(t, ir.expand(2.0), 4.0, 1.0),
                None => ui.p.icon(it.icon(), ir.inset(1.0), palette::TEXT_DIM),
            }
            let age = fmt_age(it.at.elapsed());
            let age_st = TextStyle::new(11.0, palette::TEXT_FAINT).right();
            let tx = row.x + 40.0;
            ui.p.text(
                &it.summary(),
                Rect::new(tx, row.y, row.right() - tx - 70.0, row.h),
                &TextStyle::new(12.5, palette::TEXT),
            );
            if hover {
                ui.hot = true;
                ui.p.icon(Icon::Copy, Rect::new(row.right() - 28.0, row.cy() - 8.0, 16.0, 16.0), palette::TEXT_DIM);
            } else {
                ui.p.text(&age, Rect::new(row.right() - 70.0, row.y, 60.0, row.h), &age_st);
            }
            if copyable && ui.clicked(row) {
                copy_idx = Some(i);
            }
        }
        if let Some(i) = copy_idx {
            self.copy_back(i, &mut peeks);
        }
        for p in peeks {
            ui.fx.peek(p);
        }
    }
}

fn fmt_age(d: Duration) -> String {
    let s = d.as_secs();
    match s {
        0..=59 => "now".into(),
        60..=3599 => format!("{}m ago", s / 60),
        _ => format!("{}h ago", s / 3600),
    }
}

fn file_name(p: &str) -> String {
    p.rsplit(['\\', '/']).next().unwrap_or(p).to_string()
}

/// Read the clipboard. Returns the item and, for images, a BMP file to
/// decode off-thread.
fn read_clipboard(hwnd: HWND) -> Option<(Kind, Option<Vec<u8>>)> {
    unsafe {
        let exclude = RegisterClipboardFormatW(w!("ExcludeClipboardContentFromMonitorProcessing"));
        let viewer_ignore = RegisterClipboardFormatW(w!("Clipboard Viewer Ignore"));
        let history = RegisterClipboardFormatW(w!("CanIncludeInClipboardHistory"));
        if IsClipboardFormatAvailable(exclude).is_ok() || IsClipboardFormatAvailable(viewer_ignore).is_ok() {
            return None;
        }
        // another app may hold the clipboard for a moment
        let mut opened = false;
        for _ in 0..5 {
            if OpenClipboard(Some(hwnd)).is_ok() {
                opened = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(15));
        }
        if !opened {
            return None;
        }
        let result = (|| {
            if let Ok(h) = GetClipboardData(history) {
                if let Some(v) = with_global(h, |b| b.get(0..4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))) {
                    if v == Some(0) {
                        return None;
                    }
                }
            }
            if let Ok(h) = GetClipboardData(CF_HDROP.0 as u32) {
                let drop = HDROP(h.0);
                let n = DragQueryFileW(drop, u32::MAX, None);
                let mut files = Vec::new();
                for i in 0..n.min(200) {
                    let len = DragQueryFileW(drop, i, None) as usize;
                    let mut buf = vec![0u16; len + 1];
                    DragQueryFileW(drop, i, Some(&mut buf));
                    files.push(String::from_utf16_lossy(&buf[..len]));
                }
                if !files.is_empty() {
                    return Some((Kind::Files(files), None));
                }
            }
            if let Ok(h) = GetClipboardData(CF_UNICODETEXT.0 as u32) {
                let text = with_global(h, |b| {
                    let words: Vec<u16> = b
                        .as_chunks::<2>()
                        .0
                        .iter()
                        .map(|&c| u16::from_le_bytes(c))
                        .take_while(|&c| c != 0)
                        .take(20_000)
                        .collect();
                    String::from_utf16_lossy(&words)
                })?;
                if text.trim().is_empty() {
                    return None;
                }
                return Some((Kind::Text(text), None));
            }
            let dib = GetClipboardData(CF_DIBV5.0 as u32).or_else(|_| GetClipboardData(CF_DIB.0 as u32));
            if let Ok(h) = dib {
                return with_global(h, |b| {
                    let (w, hgt) = dib_size(b)?;
                    let bmp = (b.len() <= 64 * 1024 * 1024).then(|| dib_to_bmp(b)).flatten();
                    Some((Kind::Image { w, h: hgt }, bmp))
                })
                .flatten();
            }
            None
        })();
        let _ = CloseClipboard();
        result
    }
}

fn dib_size(b: &[u8]) -> Option<(u32, u32)> {
    if b.len() < 16 {
        return None;
    }
    let w = i32::from_le_bytes(b[4..8].try_into().ok()?);
    let h = i32::from_le_bytes(b[8..12].try_into().ok()?);
    Some((w.unsigned_abs(), h.unsigned_abs()))
}

/// Prefix a packed DIB with a BITMAPFILEHEADER so WIC can decode it.
fn dib_to_bmp(dib: &[u8]) -> Option<Vec<u8>> {
    if dib.len() < 40 {
        return None;
    }
    let hsize = u32::from_le_bytes(dib[0..4].try_into().ok()?);
    let bits = u16::from_le_bytes(dib[14..16].try_into().ok()?) as u32;
    let compression = u32::from_le_bytes(dib[16..20].try_into().ok()?);
    let clr_used = u32::from_le_bytes(dib[32..36].try_into().ok()?);
    let masks = if compression == 3 && hsize == 40 { 12 } else { 0 };
    let palette = if clr_used > 0 {
        clr_used
    } else if bits <= 8 {
        1 << bits
    } else {
        0
    };
    let off = 14 + hsize + masks + palette * 4;
    let mut out = Vec::with_capacity(dib.len() + 14);
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&((dib.len() + 14) as u32).to_le_bytes());
    out.extend_from_slice(&[0, 0, 0, 0]);
    out.extend_from_slice(&off.to_le_bytes());
    out.extend_from_slice(dib);
    Some(out)
}

fn set_files(hwnd: HWND, files: &[String]) -> bool {
    let header = std::mem::size_of::<DROPFILES>();
    let mut list: Vec<u16> = Vec::new();
    for f in files {
        list.extend(f.encode_utf16());
        list.push(0);
    }
    list.push(0);
    let mut bytes = vec![0u8; header];
    let df = DROPFILES { pFiles: header as u32, fWide: true.into(), ..Default::default() };
    unsafe { std::ptr::copy_nonoverlapping(&df as *const _ as *const u8, bytes.as_mut_ptr(), header) };
    bytes.extend(list.iter().flat_map(|c| c.to_le_bytes()));
    clip::set_data(hwnd, CF_HDROP.0 as u32, &bytes)
}
