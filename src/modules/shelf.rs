//! File shelf: drag files onto the notch to park them, drag them back out
//! into any folder or app later.
//!
//! Parked items are references (paths), never copies. Dragging one out is a
//! normal shell drag, so Explorer moves within a drive and copies across
//! drives exactly like it does for its own files; items whose file is gone
//! afterwards drop off the shelf. With `remember` the paths survive restarts.

use std::any::Any;
use std::path::Path;

use super::{CardSize, Cx, Module, ModuleId, SystemEvent};
use crate::bus::Bus;
use crate::gfx::{Icon, ImageData, TextStyle};
use crate::sys::{self, dragdrop};
use crate::ui::{ButtonStyle, Ui};
use crate::util::{app_dir, palette, Color, Rect};

const ID: ModuleId = "shelf";
const MAX_ITEMS: usize = 24;
const TILE_W: f32 = 84.0;
const TILE_H: f32 = 86.0;
const GAP: f32 = 8.0;
const THUMB: f32 = 42.0;
/// Pointer travel (DIPs) that turns a press into a drag.
const DRAG_SLOP: f32 = 5.0;

struct Item {
    path: String,
    name: String,
    thumb: Option<ImageData>,
}

struct Thumb(String, Option<ImageData>);

pub struct Shelf {
    items: Vec<Item>,
    drop_hover: bool,
    remember: bool,
    bus: Option<Bus>,
}

impl Shelf {
    pub fn new() -> Self {
        Self { items: Vec::new(), drop_hover: false, remember: true, bus: None }
    }

    fn add(&mut self, paths: &[String]) {
        let mut fresh = Vec::new();
        for p in paths {
            if self.items.iter().any(|i| i.path.eq_ignore_ascii_case(p)) {
                continue;
            }
            self.items.push(Item { path: p.clone(), name: file_name(p), thumb: None });
            fresh.push(p.clone());
        }
        if self.items.len() > MAX_ITEMS {
            self.items.drain(..self.items.len() - MAX_ITEMS);
        }
        self.load_thumbs(fresh);
        self.save();
    }

    fn remove(&mut self, idx: usize) {
        if idx < self.items.len() {
            self.items.remove(idx);
            self.save();
        }
    }

    /// Forget items whose file was moved away or deleted.
    fn prune(&mut self) -> bool {
        let n = self.items.len();
        self.items.retain(|i| Path::new(&i.path).exists());
        let changed = self.items.len() != n;
        if changed {
            self.save();
        }
        changed
    }

    fn paths(&self) -> Vec<String> {
        self.items.iter().map(|i| i.path.clone()).collect()
    }

    fn load_thumbs(&self, paths: Vec<String>) {
        let Some(bus) = self.bus.clone() else { return };
        if paths.is_empty() {
            return;
        }
        let _ = std::thread::Builder::new().name("shelf-thumbs".into()).spawn(move || {
            use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};
            unsafe {
                let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            }
            for p in paths {
                let t = dragdrop::thumbnail(&p, 96);
                bus.to_module(ID, Thumb(p, t));
            }
            unsafe { CoUninitialize() };
        });
    }

    fn save(&self) {
        if !self.remember {
            return;
        }
        let json = serde_json::to_string_pretty(&self.paths()).unwrap_or_default();
        let _ = std::fs::write(store_path(), json);
    }
}

fn store_path() -> std::path::PathBuf {
    app_dir().join("shelf.json")
}

fn file_name(p: &str) -> String {
    let t = p.trim_end_matches(['\\', '/']);
    t.rsplit(['\\', '/']).next().filter(|s| !s.is_empty()).unwrap_or(t).to_string()
}

/// True when a press that started inside `r` has moved far enough to be a drag.
fn drag_started(ui: &Ui, r: Rect) -> bool {
    match (ui.input.down, ui.input.mouse) {
        (Some((dx, dy)), Some((mx, my))) => {
            ui.interactive && r.contains(dx, dy) && ((mx - dx).powi(2) + (my - dy).powi(2)).sqrt() > DRAG_SLOP
        }
        _ => false,
    }
}

impl Module for Shelf {
    fn id(&self) -> ModuleId {
        ID
    }
    fn title(&self) -> &str {
        "Shelf"
    }
    fn icon(&self) -> Icon {
        Icon::Package
    }

    fn start(&mut self, cx: &mut Cx) {
        self.bus = Some(cx.bus.clone());
        self.remember = cx.cfg.shelf.remember;
        if self.remember && self.items.is_empty() {
            let saved: Vec<String> = std::fs::read_to_string(store_path())
                .ok()
                .and_then(|t| serde_json::from_str(&t).ok())
                .unwrap_or_default();
            let existing: Vec<String> = saved.into_iter().filter(|p| Path::new(p).exists()).collect();
            self.add(&existing);
        }
    }

    fn on_message(&mut self, msg: Box<dyn Any + Send>, cx: &mut Cx) {
        if let Ok(t) = msg.downcast::<Thumb>() {
            if let Some(it) = self.items.iter_mut().find(|i| i.path == t.0) {
                it.thumb = t.1;
                cx.fx.redraw = true;
            }
        }
    }

    fn on_system(&mut self, ev: &SystemEvent, cx: &mut Cx) {
        match ev {
            SystemEvent::DragHover(on) => {
                self.drop_hover = *on;
                cx.fx.redraw = true;
            }
            SystemEvent::Dropped(files) => {
                self.drop_hover = false;
                self.add(files);
                cx.fx.layout_changed = true;
            }
            SystemEvent::DragOutDone | SystemEvent::Expanded(true) if self.prune() => {
                cx.fx.layout_changed = true;
            }
            SystemEvent::ConfigReloaded => {
                self.remember = cx.cfg.shelf.remember;
                if !self.remember {
                    let _ = std::fs::remove_file(store_path());
                } else {
                    self.save();
                }
            }
            _ => {}
        }
    }

    fn card(&self) -> Option<CardSize> {
        (!self.items.is_empty()).then_some(CardSize::Small)
    }

    fn draw_card(&mut self, ui: &mut Ui, r: Rect) {
        if drag_started(ui, r) {
            ui.fx.drag_files = Some(self.paths());
        }
        if ui.card(r, true) {
            ui.fx.open_page = Some(ID);
        }
        let inner = r.inset_xy(12.0, 0.0);
        ui.p.icon(Icon::Package, Rect::new(inner.x, r.cy() - 9.0, 18.0, 18.0), palette::YELLOW);
        let label = match self.items.len() {
            1 => self.items[0].name.clone(),
            n => format!("{n} items"),
        };
        // up to three overlapping thumbnails on the right
        let mut x = inner.right();
        let s = (r.h - 12.0).min(26.0);
        for it in self.items.iter().rev().take(3) {
            x -= s * 0.72;
            let tr = Rect::new(x - s * 0.28, r.cy() - s * 0.5, s, s);
            match &it.thumb {
                Some(img) => ui.p.image(img, tr, 5.0, 1.0),
                None => ui.p.icon(Icon::File, tr, palette::TEXT_DIM),
            }
        }
        let tx = inner.x + 28.0;
        ui.p.text(&label, Rect::new(tx, r.y, x - tx - 12.0, r.h), &TextStyle::new(12.5, palette::TEXT));
    }

    fn has_page(&self) -> bool {
        true
    }

    fn page_height(&self, w: f32) -> f32 {
        if self.items.is_empty() {
            return 120.0;
        }
        let cols = (((w + GAP) / (TILE_W + GAP)).floor() as usize).max(1);
        let rows = self.items.len().div_ceil(cols);
        28.0 + rows as f32 * (TILE_H + GAP)
    }

    fn draw_page(&mut self, ui: &mut Ui, r: Rect) {
        let (head, body) = r.split_top(24.0, 4.0);
        ui.caption("SHELF", Rect::new(head.x, head.y, 200.0, head.h));
        let accent = palette::YELLOW;

        // drop zone highlight while files hover over the notch
        if self.drop_hover {
            let zone = Rect::new(body.x, body.y, body.w, body.h - GAP * 0.5);
            ui.p.fill_rounded(zone, 14.0, accent.with_a(0.08));
            ui.p.stroke_rounded(zone.inset(0.75), 14.0, 1.5, accent.with_a(0.7));
        }
        if self.items.is_empty() {
            let msg =
                if self.drop_hover { "Drop to park files here" } else { "Drag files onto the notch to park them" };
            ui.empty_state(body, Icon::Package, msg);
            return;
        }

        // header actions
        let bs = ButtonStyle::filled(Color::white(0.08)).fg(palette::TEXT_DIM);
        let clear = Rect::new(head.right() - 64.0, head.y + 1.0, 64.0, 22.0);
        if ui.pill_button(clear, "Clear", None, bs) {
            self.items.clear();
            self.save();
            ui.fx.layout_changed = true;
            return;
        }
        if self.items.len() > 1 {
            let all = Rect::new(clear.x - 96.0, head.y + 1.0, 90.0, 22.0);
            if drag_started(ui, all) {
                ui.fx.drag_files = Some(self.paths());
            }
            ui.pill_button(all, "Drag all", Some(Icon::Copy), bs);
        }

        let cols = (((body.w + GAP) / (TILE_W + GAP)).floor() as usize).max(1);
        let mut remove = None;
        for (i, it) in self.items.iter().enumerate() {
            let (c, row) = (i % cols, i / cols);
            let tile =
                Rect::new(body.x + c as f32 * (TILE_W + GAP), body.y + row as f32 * (TILE_H + GAP), TILE_W, TILE_H);
            let hover = ui.hovered(tile);
            if hover {
                ui.hot = true;
                ui.p.fill_rounded(tile, 12.0, palette::CARD_HOVER);
            }
            let th = Rect::new(tile.cx() - THUMB * 0.5, tile.y + 10.0, THUMB, THUMB);
            match &it.thumb {
                Some(img) => {
                    // keep the aspect ratio inside the thumbnail square
                    let k = (THUMB / img.width.max(1) as f32).min(THUMB / img.height.max(1) as f32);
                    let (w, h) = (img.width as f32 * k, img.height as f32 * k);
                    ui.p.image(img, Rect::new(th.cx() - w * 0.5, th.cy() - h * 0.5, w, h), 6.0, 1.0);
                }
                None => ui.p.icon(Icon::File, th.inset(4.0), palette::TEXT_DIM),
            }
            let name = Rect::new(tile.x + 4.0, th.bottom() + 6.0, tile.w - 8.0, 18.0);
            ui.p.text(&it.name, name, &TextStyle::new(11.5, palette::TEXT).center());

            let x = Rect::new(tile.right() - 22.0, tile.y + 4.0, 18.0, 18.0);
            let mut on_x = false;
            if hover {
                on_x = ui.hovered(x);
                if ui.icon_button(x, Icon::Close, ButtonStyle::filled(Color::white(0.14)).scale(0.6)) {
                    remove = Some(i);
                }
            }
            if drag_started(ui, tile) && !on_x {
                ui.fx.drag_files = Some(vec![it.path.clone()]);
            } else if !on_x && ui.clicked(tile) {
                sys::shell_open(&it.path);
            }
        }
        if let Some(i) = remove {
            self.remove(i);
            ui.fx.layout_changed = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::file_name;

    #[test]
    fn names() {
        assert_eq!(file_name(r"C:\Users\me\report.pdf"), "report.pdf");
        assert_eq!(file_name(r"C:\Users\me\Photos\"), "Photos");
        assert_eq!(file_name(r"D:\"), "D:");
    }
}
