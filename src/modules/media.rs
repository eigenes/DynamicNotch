//! Now playing — Windows Global System Media Transport Controls (the same
//! source the Windows volume flyout uses). Works with Spotify, browsers,
//! Media Player, VLC, ... anything that integrates with SMTC.
//!
//! A worker thread owns the WinRT session manager, subscribes to its
//! events and pushes immutable snapshots to the UI thread.

use std::any::Any;
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use windows::core::Interface;
use windows::Foundation::TypedEventHandler;
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSession as Session, GlobalSystemMediaTransportControlsSessionManager as Manager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus as Status,
};
use windows::Storage::Streams::DataReader;
use windows::Win32::System::SystemInformation::GetSystemTimeAsFileTime;
use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};
use windows_future::IAsyncOperation;

use super::{Activity, CardSize, Cx, Module, ModuleId, Peek, Slot, SystemEvent};
use crate::bus::Bus;
use crate::gfx::{image, Icon, ImageData, TextStyle};
use crate::ui::{ButtonStyle, Ui};
use crate::util::{format_duration, palette, Color, Rect};

const ID: ModuleId = "media";

#[derive(Clone, Debug, Default)]
struct Snapshot {
    active: bool,
    app: String,
    title: String,
    artist: String,
    playing: bool,
    /// Position at `stamp`, seconds.
    position: f64,
    duration: f64,
    can_seek: bool,
    can_next: bool,
    can_prev: bool,
    art: Option<ImageData>,
}

enum Cmd {
    Refresh,
    PlayPause,
    Next,
    Prev,
    Seek(f64),
    Quit,
}

pub struct Media {
    snap: Snapshot,
    stamp: Instant,
    tx: Option<Sender<Cmd>>,
    paused_at: Option<Instant>,
    expanded: bool,
    linger: Duration,
    first: bool,
    /// Optimistic UI state after a click, until the worker confirms.
    pending_playing: Option<(bool, Instant)>,
}

impl Media {
    pub fn new() -> Self {
        Self {
            snap: Snapshot::default(),
            stamp: Instant::now(),
            tx: None,
            paused_at: None,
            expanded: false,
            linger: Duration::from_secs(20),
            first: true,
            pending_playing: None,
        }
    }

    fn send(&self, c: Cmd) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(c);
        }
    }

    fn playing(&self) -> bool {
        match self.pending_playing {
            Some((p, t)) if t.elapsed() < Duration::from_millis(1500) => p,
            _ => self.snap.playing,
        }
    }

    fn position(&self) -> f64 {
        let mut p = self.snap.position;
        if self.snap.playing {
            p += self.stamp.elapsed().as_secs_f64();
        }
        if self.snap.duration > 0.0 {
            p = p.min(self.snap.duration);
        }
        p.max(0.0)
    }

    fn visible(&self) -> bool {
        self.snap.active && (self.playing() || self.paused_at.map_or(false, |t| t.elapsed() < self.linger))
    }

    fn accent(&self) -> Color {
        self.snap.art.as_ref().and_then(|a| a.accent).unwrap_or(Color::white(0.9))
    }

    fn toggle(&mut self) {
        let p = !self.playing();
        self.pending_playing = Some((p, Instant::now()));
        self.send(Cmd::PlayPause);
    }
}

impl Module for Media {
    fn id(&self) -> ModuleId {
        ID
    }
    fn title(&self) -> &str {
        "Media"
    }
    fn icon(&self) -> Icon {
        Icon::Music
    }

    fn start(&mut self, cx: &mut Cx) {
        self.linger = Duration::from_secs(cx.cfg.media.linger_after_pause_secs as u64);
        let (tx, rx) = channel();
        let bus = cx.bus.clone();
        let tx2 = tx.clone();
        let _ = std::thread::Builder::new().name("media".into()).spawn(move || worker(rx, tx2, bus));
        self.tx = Some(tx);
    }

    fn stop(&mut self) {
        self.send(Cmd::Quit);
        self.tx = None;
        self.snap = Snapshot::default();
    }

    fn on_message(&mut self, msg: Box<dyn Any + Send>, cx: &mut Cx) {
        let Ok(s) = msg.downcast::<Snapshot>() else { return };
        let s = *s;
        let track_changed = s.title != self.snap.title || s.artist != self.snap.artist;
        if self.snap.playing && !s.playing {
            self.paused_at = Some(Instant::now());
        }
        if s.playing {
            self.paused_at = None;
        }
        if !self.first
            && track_changed
            && s.active
            && s.playing
            && !s.title.is_empty()
            && cx.cfg.media.peek_on_track_change
            && !self.expanded
        {
            let p = Peek::new(
                Icon::Music,
                s.art.as_ref().and_then(|a| a.accent).unwrap_or(palette::PINK),
                &s.title,
                &s.artist,
            )
            .image(s.art.clone())
            .key("media")
            .duration_ms(3000);
            cx.fx.peek(p);
        }
        self.first = false;
        // Keep the optimistic play/pause icon until the player confirms the
        // change; snapshots read right after the click are often stale and
        // would make the icon flicker (pause → play → pause → play).
        if let Some((want, at)) = self.pending_playing {
            if s.playing == want || at.elapsed() > Duration::from_millis(1500) {
                self.pending_playing = None;
            }
        }
        self.snap = s;
        self.stamp = Instant::now();
        cx.fx.redraw = true;
    }

    fn on_system(&mut self, ev: &SystemEvent, cx: &mut Cx) {
        match ev {
            SystemEvent::Expanded(e) => self.expanded = *e,
            SystemEvent::ConfigReloaded => {
                self.linger = Duration::from_secs(cx.cfg.media.linger_after_pause_secs as u64)
            }
            SystemEvent::Command { verb, .. } => match verb.as_str() {
                "playpause" | "play" | "pause" => self.toggle(),
                "next" => self.send(Cmd::Next),
                "prev" | "previous" => self.send(Cmd::Prev),
                _ => {}
            },
            _ => {}
        }
    }

    fn next_tick(&self) -> Option<Instant> {
        if self.expanded && self.snap.active && self.snap.playing {
            return Some(Instant::now() + Duration::from_millis(500));
        }
        // when the paused linger expires the activity disappears
        self.paused_at.map(|t| t + self.linger).filter(|t| *t > Instant::now())
    }

    fn on_tick(&mut self, cx: &mut Cx) {
        cx.fx.redraw = true;
    }

    fn activity(&self) -> Option<Activity> {
        if !self.visible() {
            return None;
        }
        let left = match &self.snap.art {
            Some(img) => Slot::Image(img.clone()),
            None => Slot::Icon(Icon::Music, self.accent()),
        };
        Some(Activity {
            priority: if self.playing() { 50 } else { 10 },
            left,
            right: Slot::Bars { color: self.accent(), playing: self.playing() },
            wing: 1.05,
            page: None,
        })
    }

    fn card(&self) -> Option<CardSize> {
        if self.snap.active {
            Some(CardSize::Large)
        } else {
            None
        }
    }

    fn draw_card(&mut self, ui: &mut Ui, r: Rect) {
        ui.card(r, false);
        let pad = 12.0;
        let art_s = (r.h - 2.0 * pad).min(r.w * 0.42);
        let art = Rect::new(r.x + pad, r.y + pad, art_s, art_s);
        match &self.snap.art {
            Some(img) => ui.p.image(img, art, 14.0, 1.0),
            None => {
                ui.p.fill_rounded(art, 14.0, Color::white(0.08));
                ui.p.icon(Icon::Music, art.center_square(art_s * 0.4), palette::TEXT_FAINT);
            }
        }
        let col = Rect::new(art.right() + 14.0, r.y + pad, r.right() - art.right() - 14.0 - pad, r.h - 2.0 * pad);
        let title = if self.snap.title.is_empty() { "Unknown title" } else { &self.snap.title };
        ui.p.text(title, Rect::new(col.x, col.y + 2.0, col.w, 20.0), &TextStyle::new(14.5, palette::TEXT).semibold());
        let sub = if !self.snap.artist.is_empty() {
            self.snap.artist.clone()
        } else if self.snap.app != title {
            self.snap.app.clone()
        } else {
            String::new()
        };
        ui.p.text(&sub, Rect::new(col.x, col.y + 22.0, col.w, 18.0), &TextStyle::new(12.5, palette::TEXT_DIM));

        // progress
        let bar_y = col.y + 50.0;
        let pos = self.position();
        let dur = self.snap.duration;
        let bar = Rect::new(col.x, bar_y, col.w, 4.0);
        let frac = if dur > 0.0 { (pos / dur) as f32 } else { 0.0 };
        let hit = bar.inset_xy(0.0, -8.0);
        let hover = self.snap.can_seek && dur > 0.0 && ui.hovered(hit);
        let bar_draw = if hover { Rect::new(bar.x, bar.y - 1.0, bar.w, 6.0) } else { bar };
        ui.progress(bar_draw, frac, Color::white(0.85));
        if self.snap.can_seek && dur > 0.0 {
            if let Some(f) = ui.clicked_at(hit) {
                let target = f as f64 * dur;
                self.snap.position = target;
                self.stamp = Instant::now();
                self.send(Cmd::Seek(target));
            }
        }
        if dur > 0.0 {
            let ts = TextStyle::new(10.5, palette::TEXT_FAINT).display();
            ui.p.text(&format_duration(pos), Rect::new(col.x, bar_y + 7.0, 60.0, 14.0), &ts);
            ui.p.text(
                &format!("-{}", format_duration(dur - pos)),
                Rect::new(col.right() - 60.0, bar_y + 7.0, 60.0, 14.0),
                &ts.right(),
            );
        }

        // transport controls
        let cy = col.bottom() - 17.0;
        let big = 36.0;
        let small = 30.0;
        let cx = col.cx();
        let style = ButtonStyle::default().scale(0.5);
        let prev = Rect::centered(cx - big * 0.5 - 14.0 - small * 0.5, cy, small, small);
        let play = Rect::centered(cx, cy, big, big);
        let next = Rect::centered(cx + big * 0.5 + 14.0 + small * 0.5, cy, small, small);
        let dim = |on: bool| if on { palette::TEXT } else { palette::TEXT_FAINT };
        if ui.icon_button(prev, Icon::Prev, style.fg(dim(self.snap.can_prev))) {
            self.send(Cmd::Prev);
        }
        let pp = if self.playing() { Icon::Pause } else { Icon::Play };
        if ui.icon_button(play, pp, ButtonStyle::default().scale(0.56)) {
            self.toggle();
        }
        if ui.icon_button(next, Icon::Next, style.fg(dim(self.snap.can_next))) {
            self.send(Cmd::Next);
        }
    }
}

// ---------------------------------------------------------------------------
// Worker
// ---------------------------------------------------------------------------

fn worker(rx: Receiver<Cmd>, tx: Sender<Cmd>, bus: Bus) {
    unsafe {
        let _ = RoInitialize(RO_INIT_MULTITHREADED);
    }
    let mgr = match Manager::RequestAsync().and_then(|op| op.join()) {
        Ok(m) => m,
        Err(e) => {
            crate::log!("media: session manager unavailable: {e}");
            return;
        }
    };
    let ev_tx = tx.clone();
    let _ = mgr.CurrentSessionChanged(&TypedEventHandler::new(move |_, _| {
        let _ = ev_tx.send(Cmd::Refresh);
        Ok(())
    }));
    let ev_tx = tx.clone();
    let _ = mgr.SessionsChanged(&TypedEventHandler::new(move |_, _| {
        let _ = ev_tx.send(Cmd::Refresh);
        Ok(())
    }));

    let mut session: Option<(Session, [i64; 3])> = None;
    let mut last_key = String::new();
    let mut last_art: Option<ImageData> = None;
    let mut refresh = true;

    loop {
        if refresh {
            refresh = false;
            // (re)bind to the current session's events
            let current = mgr.GetCurrentSession().ok();
            let same = match (&session, &current) {
                (Some((a, _)), Some(b)) => a == b,
                (None, None) => true,
                _ => false,
            };
            if !same {
                if let Some((s, t)) = session.take() {
                    let _ = s.RemoveMediaPropertiesChanged(t[0]);
                    let _ = s.RemovePlaybackInfoChanged(t[1]);
                    let _ = s.RemoveTimelinePropertiesChanged(t[2]);
                }
                if let Some(s) = current {
                    let t0 = s.MediaPropertiesChanged(&refresh_handler(&tx)).unwrap_or(0);
                    let t1 = s.PlaybackInfoChanged(&refresh_handler(&tx)).unwrap_or(0);
                    let t2 = s.TimelinePropertiesChanged(&refresh_handler(&tx)).unwrap_or(0);
                    session = Some((s, [t0, t1, t2]));
                }
            }
            let snap = match &session {
                Some((s, _)) => read_snapshot(s, &mut last_key, &mut last_art),
                None => {
                    last_key.clear();
                    last_art = None;
                    Snapshot::default()
                }
            };
            bus.to_module(ID, snap);
        }

        // wait for the next event / command; coalesce bursts of events
        let cmd = match rx.recv() {
            Ok(c) => c,
            Err(_) => return,
        };
        let mut cmds = vec![cmd];
        loop {
            match rx.recv_timeout(Duration::from_millis(40)) {
                Ok(c) => cmds.push(c),
                Err(RecvTimeoutError::Timeout) => break,
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
        for c in cmds {
            let s = session.as_ref().map(|(s, _)| s);
            match c {
                Cmd::Refresh => refresh = true,
                Cmd::Quit => return,
                Cmd::PlayPause => {
                    if let Some(s) = s {
                        let _ = s.TryTogglePlayPauseAsync().and_then(|o| o.join());
                    }
                    refresh = true;
                }
                Cmd::Next => {
                    if let Some(s) = s {
                        let _ = s.TrySkipNextAsync().and_then(|o| o.join());
                    }
                }
                Cmd::Prev => {
                    if let Some(s) = s {
                        let _ = s.TrySkipPreviousAsync().and_then(|o| o.join());
                    }
                }
                Cmd::Seek(secs) => {
                    if let Some(s) = s {
                        let ticks = (secs * 10_000_000.0) as i64;
                        let _ = s.TryChangePlaybackPositionAsync(ticks).and_then(|o| o.join());
                    }
                    refresh = true;
                }
            }
        }
    }
}

fn refresh_handler<S, A>(tx: &Sender<Cmd>) -> TypedEventHandler<S, A>
where
    S: windows::core::RuntimeType + 'static,
    A: windows::core::RuntimeType + 'static,
{
    let t = tx.clone();
    TypedEventHandler::new(move |_, _| {
        let _ = t.send(Cmd::Refresh);
        Ok(())
    })
}

fn read_snapshot(s: &Session, last_key: &mut String, last_art: &mut Option<ImageData>) -> Snapshot {
    let mut snap = Snapshot { active: true, ..Default::default() };
    snap.app = friendly_app(&s.SourceAppUserModelId().map(|h| h.to_string()).unwrap_or_default());
    if let Ok(props) = s.TryGetMediaPropertiesAsync().and_then(|o| o.join()) {
        snap.title = props.Title().map(|h| h.to_string()).unwrap_or_default();
        snap.artist = props.Artist().map(|h| h.to_string()).unwrap_or_default();
        let key = format!("{}\u{1}{}\u{1}{}", snap.app, snap.title, snap.artist);
        if key != *last_key || last_art.is_none() {
            *last_art = props.Thumbnail().ok().and_then(|t| load_thumbnail(&t));
            *last_key = key;
        }
        snap.art = last_art.clone();
    }
    if let Ok(info) = s.GetPlaybackInfo() {
        snap.playing = info.PlaybackStatus().map(|st| st == Status::Playing).unwrap_or(false);
        if let Ok(c) = info.Controls() {
            snap.can_next = c.IsNextEnabled().unwrap_or(false);
            snap.can_prev = c.IsPreviousEnabled().unwrap_or(false);
            snap.can_seek = c.IsPlaybackPositionEnabled().unwrap_or(false);
        }
    }
    if let Ok(tl) = s.GetTimelineProperties() {
        let start = tl.StartTime().map(|t| t.Duration).unwrap_or(0);
        let end = tl.EndTime().map(|t| t.Duration).unwrap_or(0);
        let pos = tl.Position().map(|t| t.Duration).unwrap_or(0);
        snap.duration = ((end - start) as f64 / 1e7).max(0.0);
        let mut p = (pos - start) as f64 / 1e7;
        // The timeline is a snapshot from `LastUpdatedTime`; extrapolate.
        if snap.playing {
            if let Ok(updated) = tl.LastUpdatedTime() {
                let now = unsafe { GetSystemTimeAsFileTime() };
                let now = ((now.dwHighDateTime as i64) << 32) | now.dwLowDateTime as i64;
                let dt = (now - updated.UniversalTime) as f64 / 1e7;
                if (0.0..3600.0).contains(&dt) {
                    p += dt;
                }
            }
        }
        snap.position = p.max(0.0);
    }
    snap
}

fn load_thumbnail(r: &windows::Storage::Streams::IRandomAccessStreamReference) -> Option<ImageData> {
    let stream = r.OpenReadAsync().ok()?.join().ok()?;
    let size = stream.Size().ok()? as u32;
    if size == 0 || size > 16 * 1024 * 1024 {
        return None;
    }
    let reader = DataReader::CreateDataReader(&stream).ok()?;
    let op: IAsyncOperation<u32> = reader.LoadAsync(size).ok()?.cast().ok()?;
    let n = op.join().ok()?;
    let mut buf = vec![0u8; n as usize];
    reader.ReadBytes(&mut buf).ok()?;
    image::decode(&buf, 256).map(crop_letterbox)
}

/// Browser thumbnails are often 16:9 with the square cover in the middle
/// and flat bars on the sides; crop to the center square in that case.
fn crop_letterbox(img: ImageData) -> ImageData {
    let (w, h) = (img.width, img.height);
    if w <= h + h / 10 {
        return img;
    }
    let side = h;
    let x0 = (w - side) / 2;
    let mut out = Vec::with_capacity((side * side * 4) as usize);
    for y in 0..side {
        let row = ((y * w + x0) * 4) as usize;
        out.extend_from_slice(&img.pixels[row..row + (side * 4) as usize]);
    }
    ImageData::from_pbgra(side, side, out)
}

fn friendly_app(aumid: &str) -> String {
    // Packaged apps: "Publisher.AppName_hash!EntryPoint" → use the package name.
    let s = aumid.split_once('!').map_or(aumid, |(pkg, _)| pkg);
    let s = s.trim_end_matches(".exe").trim_end_matches(".EXE");
    let s = s.split('_').next().unwrap_or(s);
    let s = s.rsplit('.').next().unwrap_or(s);
    let lower = s.to_ascii_lowercase();
    match lower.as_str() {
        "msedge" => "Microsoft Edge".into(),
        "chrome" => "Chrome".into(),
        "firefox" => "Firefox".into(),
        "spotify" | "spotifymusic" => "Spotify".into(),
        "zunemusic" | "mediaplayer" => "Media Player".into(),
        "vlc" => "VLC".into(),
        _ => {
            let mut c = s.chars();
            match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                None => String::new(),
            }
        }
    }
}
