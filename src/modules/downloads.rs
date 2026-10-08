//! Active downloads.
//!
//! Browsers don't expose download progress to other apps, so we watch the
//! Downloads folder (ReadDirectoryChangesW, event driven) for in-progress
//! files (.crdownload / .part / ...) and report size + speed; completion is
//! detected when the browser renames the file to its final name.
//!
//! Any tool can report *real* progress through the command line:
//!   dynamic-notch progress "Export video" 42   …   dynamic-notch progress "Export video" done

use std::any::Any;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows::Win32::Storage::FileSystem::*;
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::System::Threading::{CreateEventW, WaitForMultipleObjects, INFINITE};
use windows::Win32::System::IO::{GetOverlappedResult, OVERLAPPED};
use windows::Win32::UI::Shell::{FOLDERID_Downloads, SHGetKnownFolderPath, KF_FLAG_DEFAULT};

use super::{Activity, CardSize, Cx, Module, ModuleId, Peek, Slot, SystemEvent};
use crate::bus::Bus;
use crate::gfx::{Icon, TextStyle};
use crate::sys;
use crate::ui::{ButtonStyle, Ui};
use crate::util::{format_bytes, palette, Rect};

const ID: ModuleId = "downloads";
const TEMP_EXTS: [&str; 6] = [".crdownload", ".part", ".partial", ".download", ".opdownload", ".!qb"];

#[derive(Clone, Debug)]
struct Active {
    name: String,
    size: u64,
    speed: f64,
    /// 0..1 when known (external tasks), otherwise None.
    progress: Option<f32>,
    external: bool,
}

#[derive(Clone, Debug)]
struct Done {
    name: String,
    path: Option<String>,
    at: Instant,
}

enum DlMsg {
    Snapshot(Vec<Active>),
    Completed { name: String, path: String },
}

pub struct Downloads {
    browser: Vec<Active>,
    external: HashMap<String, Active>,
    recent: Vec<Done>,
    running: bool,
}

impl Downloads {
    pub fn new() -> Self {
        Self { browser: Vec::new(), external: HashMap::new(), recent: Vec::new(), running: false }
    }

    fn all(&self) -> Vec<Active> {
        let mut v: Vec<Active> = self.external.values().cloned().collect();
        v.sort_by(|a, b| a.name.cmp(&b.name));
        v.extend(self.browser.iter().cloned());
        v
    }

    fn complete(&mut self, name: String, path: Option<String>, cx: &mut Cx) {
        self.recent.retain(|d| d.name != name);
        self.recent.insert(0, Done { name: name.clone(), path, at: Instant::now() });
        self.recent.truncate(6);
        if cx.cfg.downloads.peek_on_complete {
            cx.fx.peek(
                Peek::new(Icon::Check, palette::GREEN, "Download complete", name)
                    .key("download")
                    .duration_ms(3500)
                    .page(ID),
            );
        }
        cx.fx.redraw = true;
    }
}

fn speed_text(s: f64) -> String {
    if s < 1.0 {
        "…".into()
    } else {
        format!("{}/s", format_bytes(s as u64))
    }
}

impl Module for Downloads {
    fn id(&self) -> ModuleId {
        ID
    }
    fn title(&self) -> &str {
        "Downloads"
    }
    fn icon(&self) -> Icon {
        Icon::Download
    }

    fn start(&mut self, cx: &mut Cx) {
        if self.running {
            return;
        }
        self.running = true;
        let mut folders: Vec<PathBuf> = cx.cfg.downloads.folders.iter().map(PathBuf::from).collect();
        if let Some(d) = downloads_folder() {
            folders.insert(0, d);
        }
        folders.dedup();
        let bus = cx.bus.clone();
        let _ = std::thread::Builder::new().name("downloads".into()).spawn(move || worker(folders, bus));
    }

    fn on_message(&mut self, msg: Box<dyn Any + Send>, cx: &mut Cx) {
        let Ok(m) = msg.downcast::<DlMsg>() else { return };
        match *m {
            DlMsg::Snapshot(v) => self.browser = v,
            DlMsg::Completed { name, path } => self.complete(name, Some(path), cx),
        }
        cx.fx.redraw = true;
    }

    fn on_system(&mut self, ev: &SystemEvent, cx: &mut Cx) {
        let SystemEvent::Command { verb, args } = ev else { return };
        if verb != "progress" || args.is_empty() {
            return;
        }
        let name = args[0].clone();
        let val = args.get(1).map(|s| s.to_ascii_lowercase()).unwrap_or_default();
        match val.as_str() {
            "done" | "complete" | "100" => {
                self.external.remove(&name);
                self.complete(name, None, cx);
            }
            "cancel" | "fail" | "failed" | "error" => {
                self.external.remove(&name);
                cx.fx.peek(Peek::new(Icon::Close, palette::RED, "Task stopped", name).key("download"));
            }
            v => {
                let p = v.trim_end_matches('%').parse::<f32>().ok().map(|p| (p / 100.0).clamp(0.0, 1.0));
                self.external.insert(name.clone(), Active { name, size: 0, speed: 0.0, progress: p, external: true });
            }
        }
        cx.fx.redraw = true;
    }

    fn activity(&self) -> Option<Activity> {
        let all = self.all();
        let first = all.first()?;
        let (left, right) = match first.progress {
            Some(p) => (Slot::Ring(Some(p), palette::BLUE), Slot::Text(format!("{:.0}%", p * 100.0), palette::BLUE)),
            None => (Slot::Icon(Icon::Download, palette::BLUE), Slot::Text(speed_text(first.speed), palette::BLUE)),
        };
        Some(Activity { priority: 30, left, right, wing: 2.1, page: Some(ID) })
    }

    fn card(&self) -> Option<CardSize> {
        let fresh = self.recent.first().is_some_and(|d| d.at.elapsed() < Duration::from_secs(15 * 60));
        (!self.browser.is_empty() || !self.external.is_empty() || fresh).then_some(CardSize::Small)
    }

    fn draw_card(&mut self, ui: &mut Ui, r: Rect) {
        if ui.card(r, true) {
            ui.fx.open_page = Some(ID);
        }
        let inner = r.inset_xy(12.0, 0.0);
        let is = 18.0;
        ui.p.icon(Icon::Download, Rect::new(inner.x, r.cy() - is * 0.5, is, is), palette::BLUE);
        let tx = inner.x + 28.0;
        let tw = inner.right() - tx;
        if let Some(a) = self.all().first() {
            let compact = r.h < 44.0;
            let name_r =
                if compact { Rect::new(tx, r.y, tw * 0.55, r.h) } else { Rect::new(tx, r.cy() - 16.0, tw, 16.0) };
            ui.p.text(&a.name, name_r, &TextStyle::new(12.5, palette::TEXT));
            let bar = if compact {
                Rect::new(tx + tw * 0.58, r.cy() - 2.0, tw * 0.42, 4.0)
            } else {
                Rect::new(tx, r.cy() + 6.0, tw, 4.0)
            };
            match a.progress {
                Some(p) => ui.progress(bar, p, palette::BLUE),
                None => ui.progress_indeterminate(bar, palette::BLUE),
            }
        } else if let Some(d) = self.recent.first().cloned() {
            let open = Rect::new(inner.right() - 26.0, r.cy() - 13.0, 26.0, 26.0);
            ui.p.text(&d.name, Rect::new(tx, r.y, open.x - tx - 6.0, r.h), &TextStyle::new(12.5, palette::TEXT));
            if let Some(p) = &d.path {
                if ui.icon_button(open, Icon::Folder, ButtonStyle::default().scale(0.55).fg(palette::TEXT_DIM)) {
                    sys::shell_reveal(p);
                }
            }
        }
    }

    fn has_page(&self) -> bool {
        true
    }

    fn page_height(&self, _w: f32) -> f32 {
        let n = self.all().len() + self.recent.len();
        if n == 0 {
            110.0
        } else {
            28.0 + n.min(6) as f32 * 46.0
        }
    }

    fn draw_page(&mut self, ui: &mut Ui, r: Rect) {
        let (head, body) = r.split_top(24.0, 4.0);
        ui.caption("DOWNLOADS", Rect::new(head.x, head.y, 200.0, head.h));
        let active = self.all();
        if active.is_empty() && self.recent.is_empty() {
            ui.empty_state(body, Icon::Download, "Downloads in your browser show up here");
            return;
        }
        let mut y = body.y;
        for a in active.iter().take(6) {
            let row = Rect::new(body.x, y, body.w, 42.0);
            ui.p.fill_rounded(row, 10.0, palette::CARD);
            ui.p.icon(Icon::Download, Rect::new(row.x + 10.0, row.cy() - 9.0, 18.0, 18.0), palette::BLUE);
            let tx = row.x + 38.0;
            let status = match a.progress {
                Some(p) => format!("{:.0}%", p * 100.0),
                None if a.external => "Working…".into(),
                None => format!("{} · {}", format_bytes(a.size), speed_text(a.speed)),
            };
            let sst = TextStyle::new(11.0, palette::TEXT_DIM).right();
            ui.p.text(&a.name, Rect::new(tx, row.y + 5.0, row.w - 180.0, 18.0), &TextStyle::new(12.5, palette::TEXT));
            ui.p.text(&status, Rect::new(row.right() - 150.0, row.y + 5.0, 140.0, 18.0), &sst);
            let bar = Rect::new(tx, row.y + 28.0, row.right() - tx - 12.0, 4.0);
            match a.progress {
                Some(p) => ui.progress(bar, p, palette::BLUE),
                None => ui.progress_indeterminate(bar, palette::BLUE),
            }
            y += 46.0;
        }
        let mut remove = None;
        for (i, d) in self.recent.iter().enumerate().take(6usize.saturating_sub(active.len())) {
            let row = Rect::new(body.x, y, body.w, 42.0);
            ui.p.fill_rounded(row, 10.0, palette::CARD);
            ui.p.icon(Icon::Check, Rect::new(row.x + 10.0, row.cy() - 9.0, 18.0, 18.0), palette::GREEN);
            let tx = row.x + 38.0;
            ui.p.text(&d.name, Rect::new(tx, row.y, row.w - 160.0, row.h), &TextStyle::new(12.5, palette::TEXT));
            let mut bx = row.right() - 36.0;
            let bs = ButtonStyle::default().scale(0.5).fg(palette::TEXT_DIM);
            if ui.icon_button(Rect::new(bx, row.cy() - 14.0, 28.0, 28.0), Icon::Close, bs) {
                remove = Some(i);
            }
            if let Some(p) = d.path.clone() {
                bx -= 32.0;
                if ui.icon_button(Rect::new(bx, row.cy() - 14.0, 28.0, 28.0), Icon::Folder, bs) {
                    sys::shell_reveal(&p);
                }
                bx -= 32.0;
                if ui.icon_button(Rect::new(bx, row.cy() - 14.0, 28.0, 28.0), Icon::File, bs) {
                    sys::shell_open(&p);
                }
            }
            y += 46.0;
        }
        if let Some(i) = remove {
            self.recent.remove(i);
        }
    }
}

// ---------------------------------------------------------------------------
// Worker
// ---------------------------------------------------------------------------

fn downloads_folder() -> Option<PathBuf> {
    unsafe {
        let p = SHGetKnownFolderPath(&FOLDERID_Downloads, KF_FLAG_DEFAULT, None).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const _));
        s.map(PathBuf::from)
    }
}

fn temp_stem(name: &str) -> Option<&str> {
    let lower = name.to_ascii_lowercase();
    TEMP_EXTS.iter().find(|e| lower.ends_with(*e)).map(|e| &name[..name.len() - e.len()])
}

fn display_name(file: &str) -> String {
    let stem = temp_stem(file).unwrap_or(file);
    if stem.starts_with("Unconfirmed ") {
        "Starting download…".into()
    } else {
        stem.to_string()
    }
}

struct Track {
    path: PathBuf,
    size: u64,
    last_size: u64,
    last_t: Instant,
    speed: f64,
}

struct Watch {
    dir: PathBuf,
    handle: HANDLE,
    event: HANDLE,
    ov: Box<OVERLAPPED>,
    buf: Box<[u8; 64 * 1024]>,
}

impl Watch {
    fn open(dir: &Path) -> Option<Self> {
        let w: Vec<u16> = dir.as_os_str().to_string_lossy().encode_utf16().chain(Some(0)).collect();
        unsafe {
            let handle = CreateFileW(
                PCWSTR(w.as_ptr()),
                FILE_LIST_DIRECTORY.0,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                None,
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OVERLAPPED,
                None,
            )
            .ok()?;
            let event = CreateEventW(None, false, false, None).ok()?;
            let mut s = Self {
                dir: dir.to_path_buf(),
                handle,
                event,
                ov: Box::new(OVERLAPPED::default()),
                buf: Box::new([0u8; 64 * 1024]),
            };
            s.arm().then_some(s)
        }
    }

    fn arm(&mut self) -> bool {
        *self.ov = OVERLAPPED { hEvent: self.event, ..Default::default() };
        unsafe {
            ReadDirectoryChangesW(
                self.handle,
                self.buf.as_mut_ptr() as *mut _,
                self.buf.len() as u32,
                false,
                FILE_NOTIFY_CHANGE_FILE_NAME | FILE_NOTIFY_CHANGE_SIZE | FILE_NOTIFY_CHANGE_LAST_WRITE,
                None,
                Some(&mut *self.ov),
                None,
            )
            .is_ok()
        }
    }

    /// Collect (action, file name) records of a completed read.
    fn records(&mut self) -> Vec<(u32, String)> {
        let mut out = Vec::new();
        let mut n = 0u32;
        if unsafe { GetOverlappedResult(self.handle, &*self.ov, &mut n, false) }.is_err() || n == 0 {
            return out;
        }
        let mut off = 0usize;
        loop {
            let base = &self.buf[off..];
            let next = u32::from_le_bytes(base[0..4].try_into().unwrap()) as usize;
            let action = u32::from_le_bytes(base[4..8].try_into().unwrap());
            let len = u32::from_le_bytes(base[8..12].try_into().unwrap()) as usize;
            let name: Vec<u16> = base[12..12 + len].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
            out.push((action, String::from_utf16_lossy(&name)));
            if next == 0 {
                break;
            }
            off += next;
        }
        out
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.handle);
            let _ = CloseHandle(self.event);
        }
    }
}

fn worker(folders: Vec<PathBuf>, bus: Bus) {
    let mut watches: Vec<Watch> = folders.iter().filter_map(|d| Watch::open(d)).collect();
    if watches.is_empty() {
        crate::log!("downloads: no folder could be watched");
        return;
    }
    let mut tracks: HashMap<PathBuf, Track> = HashMap::new();
    let mut pending_old: Option<PathBuf> = None;
    let mut last_sent = 0usize;
    loop {
        let events: Vec<HANDLE> = watches.iter().map(|w| w.event).collect();
        let timeout = if tracks.is_empty() { INFINITE } else { 700 };
        let r = unsafe { WaitForMultipleObjects(&events, false, timeout) };
        let mut changed = false;
        if r != WAIT_TIMEOUT {
            let i = (r.0 - WAIT_OBJECT_0.0) as usize;
            if let Some(w) = watches.get_mut(i) {
                for (action, name) in w.records() {
                    let path = w.dir.join(&name);
                    let is_temp = temp_stem(&name).is_some();
                    match FILE_ACTION(action) {
                        FILE_ACTION_ADDED | FILE_ACTION_MODIFIED if is_temp && !tracks.contains_key(&path) => {
                            tracks.insert(
                                path.clone(),
                                Track { path, size: 0, last_size: 0, last_t: Instant::now(), speed: 0.0 },
                            );
                            changed = true;
                        }
                        FILE_ACTION_RENAMED_OLD_NAME => pending_old = Some(path),
                        FILE_ACTION_RENAMED_NEW_NAME => {
                            if let Some(old) = pending_old.take() {
                                if let Some(mut t) = tracks.remove(&old) {
                                    if is_temp {
                                        t.path = path.clone();
                                        tracks.insert(path, t);
                                    } else {
                                        bus.to_module(
                                            ID,
                                            DlMsg::Completed {
                                                name: name.clone(),
                                                path: path.to_string_lossy().into(),
                                            },
                                        );
                                    }
                                    changed = true;
                                }
                            }
                        }
                        FILE_ACTION_REMOVED if is_temp && tracks.remove(&path).is_some() => {
                            // Some browsers delete the temp file and write the final one.
                            let stem = temp_stem(&name).unwrap_or(&name).to_string();
                            let fin = w.dir.join(&stem);
                            if fin.exists() && std::fs::metadata(&fin).map(|m| m.len() > 0).unwrap_or(false) {
                                bus.to_module(ID, DlMsg::Completed { name: stem, path: fin.to_string_lossy().into() });
                            }
                            changed = true;
                        }
                        _ => {}
                    }
                }
                w.arm();
            }
        }
        // refresh sizes / speeds
        let now = Instant::now();
        tracks.retain(|_, t| t.path.exists());
        for t in tracks.values_mut() {
            let size = std::fs::metadata(&t.path).map(|m| m.len()).unwrap_or(t.size);
            let dt = now.duration_since(t.last_t).as_secs_f64();
            if dt >= 0.5 {
                let inst = (size.saturating_sub(t.last_size)) as f64 / dt;
                t.speed = if t.speed == 0.0 { inst } else { t.speed * 0.6 + inst * 0.4 };
                t.last_size = size;
                t.last_t = now;
            }
            if size != t.size {
                t.size = size;
                changed = true;
            }
        }
        if changed || tracks.len() != last_sent {
            last_sent = tracks.len();
            let snap: Vec<Active> = tracks
                .values()
                .map(|t| Active {
                    name: display_name(&t.path.file_name().unwrap_or_default().to_string_lossy()),
                    size: t.size,
                    speed: t.speed,
                    progress: None,
                    external: false,
                })
                .collect();
            bus.to_module(ID, DlMsg::Snapshot(snap));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn temp_names() {
        assert_eq!(temp_stem("movie.mp4.crdownload"), Some("movie.mp4"));
        assert_eq!(temp_stem("a.zip.PART"), Some("a.zip"));
        assert_eq!(temp_stem("a.zip"), None);
        assert_eq!(display_name("Unconfirmed 123.crdownload"), "Starting download…");
    }
}
