//! The notch shell: owns the window, renderer, modules and the state
//! machine (idle → peek → expanded), turns state into spring targets and
//! draws a frame whenever something moves.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use windows::core::{Result, PCWSTR};
use windows::Win32::Foundation::{HANDLE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Globalization::{GetDateFormatEx, GetTimeFormatEx, DATE_LONGDATE, TIME_NOSECONDS};
use windows::Win32::Graphics::Gdi::{
    CreateRectRgn, EnumDisplayMonitors, GetMonitorInfoW, MonitorFromPoint, MonitorFromWindow, SetWindowRgn, HDC,
    HMONITOR, MONITORINFO, MONITOR_DEFAULTTONEAREST, MONITOR_DEFAULTTOPRIMARY,
};
use windows::Win32::System::DataExchange::AddClipboardFormatListener;
use windows::Win32::System::Power::RegisterPowerSettingNotification;
use windows::Win32::System::SystemInformation::GetLocalTime;
use windows::Win32::System::SystemServices::{GUID_ACDC_POWER_SOURCE, GUID_BATTERY_PERCENTAGE_REMAINING};
use windows::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::Shell::{
    SHQueryUserNotificationState, NIN_SELECT, QUNS_PRESENTATION_MODE, QUNS_RUNNING_D3D_FULL_SCREEN,
};
const NIN_KEYSELECT: u32 = NIN_SELECT | 0x1;
const WM_MOUSELEAVE: u32 = 0x02A3;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::anim::{lerp, Spring};
use crate::bus::{Bus, Msg};
use crate::config::{self, Config, LoadResult};
use crate::gfx::{self, icons, Gfx, Icon, Painter, Res, TextStyle};
use crate::host::Host;
use crate::hotkeys::{self, Action};
use crate::modules::{
    self, Activity, CardSize, Cx, Effects, Indicator, Key, Module, ModuleId, Peek, Slot, SystemEvent, Trailing,
};
use crate::pacer::Pacer;
use crate::sys::dragdrop::{self, DropEvent};
use crate::tray::{self, MenuState, Tray};
use crate::ui::{ButtonStyle, Input, Ui};
use crate::util::{palette, Color, Rect, SendHwnd};
use crate::window::{WM_APP_BUS, WM_APP_DRAG, WM_APP_FOREGROUND, WM_APP_FRAME, WM_APP_TRAY};
use crate::{ipc, log, sys};
use windows::Win32::UI::Input::{
    GetRawInputData, RegisterRawInputDevices, HRAWINPUT, RAWINPUT, RAWINPUTDEVICE, RAWINPUTHEADER, RIDEV_INPUTSINK,
    RID_INPUT, RIM_TYPEKEYBOARD,
};

const HEADER_H: f32 = 42.0;
const PAD: f32 = 20.0;
const BOTTOM_PAD: f32 = 18.0;
const MARGIN: f32 = 48.0;
const WIN_H: f32 = 520.0;
const HOME: ModuleId = "home";
const TIMER_WAKE: usize = 1;
const HOME_HEIGHT: f32 = 128.0;
/// With four tiles in the right column the home page grows a little.
const HOME_HEIGHT_TALL: f32 = 152.0;
const HOME_TILES: usize = 4;
/// Quiet time before GPU caches are released.
const TRIM_DELAY: Duration = Duration::from_secs(3);

static NOTCH_HWND: AtomicIsize = AtomicIsize::new(0);
/// A peek would be visible right now (not hidden, not expanded).
static PEEKS_SHOWN: AtomicBool = AtomicBool::new(false);

/// Whether a peek pushed now would actually be seen. Changes are also
/// broadcast as `SystemEvent::PeeksShown`.
pub fn peeks_shown() -> bool {
    PEEKS_SHOWN.load(Ordering::Relaxed)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mode {
    Idle,
    Peek,
    Expanded,
}

struct ActivePeek {
    peek: Peek,
    until: Instant,
}

struct Anim {
    w: Spring,
    h: Spring,
    r: Spring,
    ear: Spring,
    glass: Spring,
    compact: Spring,
    peek: Spring,
    exp: Spring,
    shown: Spring,
    /// Secondary live-activity bubble (0 hidden .. 1 fully out).
    sec: Spring,
}

impl Anim {
    fn new(w: f32, h: f32) -> Self {
        Self {
            w: Spring::new(w * 0.6, 0.45, 0.85),
            h: Spring::new(0.0, 0.45, 0.85),
            r: Spring::new(10.0, 0.45, 0.9),
            ear: Spring::new(6.0, 0.4, 1.0),
            glass: Spring::unit(0.0, 0.3, 1.0),
            compact: Spring::unit(0.0, 0.18, 1.0),
            peek: Spring::unit(0.0, 0.2, 1.0),
            exp: Spring::unit(0.0, 0.22, 1.0),
            shown: Spring::unit(0.0, 0.5, 0.8),
            sec: Spring::unit(0.0, 0.45, 0.8),
        }
        .with_h(h)
    }
    fn with_h(mut self, _h: f32) -> Self {
        self.h.snap(0.0);
        self
    }
    fn step(&mut self, dt: f32) -> bool {
        let mut m = false;
        for s in [
            &mut self.w,
            &mut self.h,
            &mut self.r,
            &mut self.ear,
            &mut self.glass,
            &mut self.compact,
            &mut self.peek,
            &mut self.exp,
            &mut self.shown,
            &mut self.sec,
        ] {
            m |= s.step(dt);
        }
        m
    }
}

struct Targets {
    w: f32,
    h: f32,
    r: f32,
    ear: f32,
    glass: f32,
    compact: bool,
    peek: bool,
    exp: bool,
    shown: bool,
    sec: bool,
}

pub struct App {
    hwnd: HWND,
    cfg: Config,
    gfx: Gfx,
    res: Res,
    host: Host,
    pacer: Pacer,
    bus: Bus,
    rx: Receiver<Msg>,
    modules: Vec<Box<dyn Module>>,
    started: Vec<bool>,
    /// Tick deadline each module asked for when the wake timer was last armed.
    /// `next_tick()` is relative to "now", so it must be cached to ever come due.
    tick_at: Vec<Option<Instant>>,

    monitor: HMONITOR,
    scale: f32,
    win_w: f32,
    win_h: f32,
    win_px: (i32, i32, i32, i32),

    mode: Mode,
    page: ModuleId,
    peek: Option<ActivePeek>,
    peek_queue: VecDeque<Peek>,

    hovering: bool,
    hover_since: Option<Instant>,
    leave_since: Option<Instant>,
    /// Opened by click/hotkey/CLI: auto-close if the mouse never comes by.
    auto_close: Option<Instant>,
    tracking: bool,
    input: Input,
    keyboard: bool,
    high_surrogate: Option<u16>,

    anim: Anim,
    last_frame: Instant,
    dirty: bool,
    /// When to release GPU caches (set when a frame burst ends).
    trim_at: Option<Instant>,
    region: Option<(i32, i32, i32, i32)>,
    region_anim: Option<Rect>,
    fs_hidden: bool,
    user_hidden: bool,
    shown_once: bool,

    tray: Option<Tray>,
    hotkeys: Vec<hotkeys::Registered>,
    msg_taskbar_created: u32,
    accent: Color,
    hooks: Vec<HWINEVENTHOOK>,
    loc_hook: Option<HWINEVENTHOOK>,
    loc_pid: u32,
    activity: Option<Activity>,
    secondary: Option<Activity>,
    indicators: Vec<Indicator>,
    clock_min: u16,
    /// Virtual key of the dictation hotkey (its release ends push-to-talk).
    dictate_vk: u16,
    /// Watched keys currently held (raw input repeats are dropped).
    raw_down: Vec<u16>,
    /// Files are being dragged over the notch (keeps it open).
    drag_hover: bool,
    /// Files to drag out once the window proc is outside the app borrow.
    pending_drag: Option<Vec<String>>,
    /// Frame statistics for the DN_DEBUG log.
    /// Frames in the current burst: (start, spring, animate, dirty).
    stats: (Option<Instant>, u32, u32, u32),
    debug: bool,
}

impl App {
    pub fn new(hwnd: HWND, cfg: Config) -> Result<Box<App>> {
        let gfx = Gfx::new()?;
        let res = Res::new(&gfx)?;
        let host = Host::new(hwnd, &gfx, 64, 64)?;
        let shwnd = SendHwnd::new(hwnd);
        let pacer = Pacer::new(shwnd);
        let (bus, rx) = Bus::new(shwnd);
        let modules = modules::all();
        let n = modules.len();
        NOTCH_HWND.store(hwnd.0 as isize, Ordering::Release);
        let anim = Anim::new(cfg.appearance.notch_width, cfg.appearance.notch_height);
        Ok(Box::new(App {
            hwnd,
            gfx,
            res,
            host,
            pacer,
            bus,
            rx,
            modules,
            started: vec![false; n],
            tick_at: vec![None; n],
            monitor: HMONITOR::default(),
            scale: 1.0,
            win_w: 0.0,
            win_h: 0.0,
            win_px: (0, 0, 0, 0),
            mode: Mode::Idle,
            page: HOME,
            peek: None,
            peek_queue: VecDeque::new(),
            hovering: false,
            hover_since: None,
            leave_since: None,
            auto_close: None,
            tracking: false,
            input: Input::default(),
            keyboard: false,
            high_surrogate: None,
            anim,
            last_frame: Instant::now(),
            dirty: true,
            trim_at: None,
            region: None,
            region_anim: None,
            fs_hidden: false,
            user_hidden: false,
            shown_once: false,
            tray: None,
            hotkeys: Vec::new(),
            msg_taskbar_created: unsafe { RegisterWindowMessageW(windows::core::w!("TaskbarCreated")) },
            accent: Color::white(0.92),
            hooks: Vec::new(),
            loc_hook: None,
            loc_pid: 0,
            activity: None,
            secondary: None,
            indicators: Vec::new(),
            clock_min: 99,
            dictate_vk: 0,
            raw_down: Vec::new(),
            drag_hover: false,
            pending_drag: None,
            stats: (None, 0, 0, 0),
            debug: std::env::var_os("DN_DEBUG").is_some(),
            cfg,
        }))
    }

    /// Second-stage init once the app is reachable from the window proc.
    pub fn start(&mut self) {
        self.place();
        self.tray = Some(Tray::new(self.hwnd));
        self.register_hotkeys();
        unsafe {
            let _ = AddClipboardFormatListener(self.hwnd);
            let h = HANDLE(self.hwnd.0);
            let _ = RegisterPowerSettingNotification(h, &GUID_ACDC_POWER_SOURCE, DEVICE_NOTIFY_WINDOW_HANDLE);
            let _ =
                RegisterPowerSettingNotification(h, &GUID_BATTERY_PERCENTAGE_REMAINING, DEVICE_NOTIFY_WINDOW_HANDLE);
            let fg = SetWinEventHook(
                EVENT_SYSTEM_FOREGROUND,
                EVENT_SYSTEM_FOREGROUND,
                None,
                Some(win_event_cb),
                0,
                0,
                WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
            );
            if !fg.is_invalid() {
                self.hooks.push(fg);
            }
            // Keyboard raw input in the background: lock-key toggles and the
            // release of the dictation hotkey. Nothing is blocked or recorded.
            let rid =
                RAWINPUTDEVICE { usUsagePage: 0x01, usUsage: 0x06, dwFlags: RIDEV_INPUTSINK, hwndTarget: self.hwnd };
            if let Err(e) = RegisterRawInputDevices(&[rid], std::mem::size_of::<RAWINPUTDEVICE>() as u32) {
                log!("raw input unavailable: {e}");
            }
        }
        dragdrop::register_target(self.hwnd, |ev| crate::window::with_app(|a| a.on_drop_event(ev)).unwrap_or(false));
        sys::watch_config(self.bus.clone());
        if self.cfg.general.start_with_windows != sys::is_autostart() {
            sys::set_autostart(self.cfg.general.start_with_windows);
        }
        self.sync_modules();
        self.on_foreground_changed();
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
        }
        self.request_frame();
        log!("started: scale={} monitor={:?}", self.scale, self.win_px);
    }

    pub fn shutdown(&mut self) {
        for (i, m) in self.modules.iter_mut().enumerate() {
            if self.started[i] {
                m.stop();
                self.started[i] = false;
            }
        }
        for h in self.hooks.drain(..) {
            unsafe {
                let _ = UnhookWinEvent(h);
            }
        }
        if let Some(h) = self.loc_hook.take() {
            unsafe {
                let _ = UnhookWinEvent(h);
            }
        }
        hotkeys::unregister_all(self.hwnd);
        dragdrop::revoke_target(self.hwnd);
        self.tray = None;
        self.pacer.stop();
    }

    // -----------------------------------------------------------------------
    // Message handling
    // -----------------------------------------------------------------------

    pub fn handle(&mut self, msg: u32, wp: WPARAM, lp: LPARAM) -> Option<LRESULT> {
        match msg {
            WM_APP_FRAME => {
                self.pacer.frame_consumed();
                self.frame();
                Some(LRESULT(0))
            }
            WM_APP_BUS => {
                self.drain_bus();
                Some(LRESULT(0))
            }
            WM_TIMER if wp.0 == TIMER_WAKE => {
                unsafe {
                    let _ = KillTimer(Some(self.hwnd), TIMER_WAKE);
                }
                self.on_wake();
                Some(LRESULT(0))
            }
            WM_MOUSEMOVE => {
                let (x, y) = self.lp_to_logical(lp);
                self.on_mouse_move(x, y);
                Some(LRESULT(0))
            }
            WM_MOUSELEAVE => {
                self.tracking = false;
                self.set_hover(false);
                self.input.mouse = None;
                self.input.down = None;
                self.request_frame();
                Some(LRESULT(0))
            }
            WM_LBUTTONDOWN => {
                let p = self.lp_to_logical(lp);
                self.input.down = Some(p);
                self.request_frame();
                Some(LRESULT(0))
            }
            WM_LBUTTONUP => {
                let p = self.lp_to_logical(lp);
                self.on_click(p);
                Some(LRESULT(0))
            }
            WM_MOUSEWHEEL => {
                let delta = ((wp.0 >> 16) & 0xFFFF) as u16 as i16;
                self.input.wheel += delta as f32 / 120.0;
                self.request_frame();
                Some(LRESULT(0))
            }
            WM_MOUSEACTIVATE => Some(LRESULT(MA_NOACTIVATE as isize)),
            WM_HOTKEY => {
                let id = wp.0 as i32;
                if let Some(h) = self.hotkeys.iter().find(|h| h.id == id) {
                    let a = h.action;
                    self.on_hotkey(a);
                } else {
                    self.broadcast(SystemEvent::Hotkey(id));
                }
                Some(LRESULT(0))
            }
            WM_APP_TRAY => {
                let ev = (lp.0 as u32) & 0xFFFF;
                if ev == NIN_SELECT || ev == NIN_KEYSELECT || ev == WM_LBUTTONUP {
                    self.toggle();
                }
                Some(LRESULT(0))
            }
            WM_APP_FOREGROUND => {
                self.on_foreground_changed();
                Some(LRESULT(0))
            }
            WM_INPUT => {
                self.on_raw_input(lp);
                None // DefWindowProc must still see WM_INPUT
            }
            WM_CLIPBOARDUPDATE => {
                self.broadcast(SystemEvent::ClipboardChanged);
                Some(LRESULT(0))
            }
            WM_POWERBROADCAST => {
                self.broadcast(SystemEvent::PowerChanged);
                Some(LRESULT(1))
            }
            WM_DISPLAYCHANGE | WM_DPICHANGED => {
                self.place();
                Some(LRESULT(0))
            }
            WM_SETTINGCHANGE => {
                if wp.0 as u32 == SPI_SETWORKAREA.0 {
                    self.place();
                }
                if let Some(t) = self.tray.as_mut() {
                    t.refresh_icon();
                }
                None
            }
            WM_COPYDATA => {
                if let Some(args) = ipc::decode(lp) {
                    self.on_ipc(args);
                }
                Some(LRESULT(1))
            }
            WM_ACTIVATE => {
                let state = (wp.0 & 0xFFFF) as u32;
                if state == WA_INACTIVE && self.keyboard {
                    self.set_keyboard(false);
                    if self.mode == Mode::Expanded && !self.hovering {
                        self.collapse();
                    }
                }
                None
            }
            WM_KEYDOWN | WM_SYSKEYDOWN => {
                if self.keyboard {
                    if let Some(k) = translate_key(wp.0 as u32) {
                        self.on_key(k);
                        return Some(LRESULT(0));
                    }
                }
                None
            }
            WM_CHAR => {
                if self.keyboard {
                    let u = wp.0 as u16;
                    let ch = if (0xD800..0xDC00).contains(&u) {
                        self.high_surrogate = Some(u);
                        None
                    } else if (0xDC00..0xE000).contains(&u) {
                        self.high_surrogate.take().and_then(|hi| char::decode_utf16([hi, u]).next()?.ok())
                    } else {
                        char::from_u32(u as u32)
                    };
                    if let Some(c) = ch {
                        if !c.is_control() {
                            self.on_key(Key::Char(c));
                        }
                    }
                    return Some(LRESULT(0));
                }
                None
            }
            WM_CLOSE => {
                self.quit();
                Some(LRESULT(0))
            }
            WM_DESTROY => {
                self.shutdown();
                unsafe { PostQuitMessage(0) };
                Some(LRESULT(0))
            }
            _ if msg == self.msg_taskbar_created && msg != 0 => {
                if let Some(t) = self.tray.as_mut() {
                    t.readd();
                }
                Some(LRESULT(0))
            }
            _ => None,
        }
    }

    fn on_raw_input(&mut self, lp: LPARAM) {
        let mut ri = RAWINPUT::default();
        let mut size = std::mem::size_of::<RAWINPUT>() as u32;
        let n = unsafe {
            GetRawInputData(
                HRAWINPUT(lp.0 as *mut _),
                RID_INPUT,
                Some(&mut ri as *mut _ as *mut _),
                &mut size,
                std::mem::size_of::<RAWINPUTHEADER>() as u32,
            )
        };
        if n == u32::MAX || ri.header.dwType != RIM_TYPEKEYBOARD.0 {
            return;
        }
        let kb = unsafe { ri.data.keyboard };
        let vk = kb.VKey;
        let watched = [VK_CAPITAL.0, VK_NUMLOCK.0, VK_SCROLL.0].contains(&vk) || (vk != 0 && vk == self.dictate_vk);
        if !watched {
            return;
        }
        let down = kb.Flags as u32 & RI_KEY_BREAK == 0;
        if down {
            if self.raw_down.contains(&vk) {
                return; // auto-repeat
            }
            self.raw_down.push(vk);
        } else {
            // A registered hotkey's key-down never reaches raw input, its key-up does.
            self.raw_down.retain(|&k| k != vk);
        }
        self.broadcast(SystemEvent::RawKey { vk, down });
    }

    /// OLE drop target callbacks (files dragged onto the notch).
    pub fn on_drop_event(&mut self, ev: DropEvent) -> bool {
        if !self.module_index("shelf").is_some_and(|i| self.started[i]) {
            return false;
        }
        match ev {
            DropEvent::Enter => {
                self.drag_hover = true;
                self.user_hidden = false;
                if self.mode != Mode::Expanded || self.page != "shelf" {
                    self.expand(Some("shelf"));
                }
                self.broadcast(SystemEvent::DragHover(true));
            }
            DropEvent::Leave => {
                self.drag_hover = false;
                self.leave_since = Some(Instant::now());
                self.broadcast(SystemEvent::DragHover(false));
            }
            DropEvent::Drop(files) => {
                self.drag_hover = false;
                // the cursor is on the notch but sends no moves until it does
                self.leave_since = None;
                self.auto_close = Some(Instant::now() + Duration::from_secs(4));
                self.broadcast(SystemEvent::DragHover(false));
                self.broadcast(SystemEvent::Dropped(files));
            }
        }
        self.request_frame();
        self.schedule_wake();
        true
    }

    /// Files to drag out (see `WM_APP_DRAG` in window.rs). The button-up ends
    /// up in the drag loop, so the press is forgotten here.
    pub fn take_drag(&mut self) -> Option<Vec<String>> {
        self.input.down = None;
        self.pending_drag.take()
    }

    pub fn drag_finished(&mut self) {
        // The drag loop held the mouse capture, so WM_MOUSELEAVE may never
        // have arrived; resync hover from the real cursor position.
        let mut pt = POINT::default();
        unsafe {
            let _ = GetCursorPos(&mut pt);
            let _ = windows::Win32::Graphics::Gdi::ScreenToClient(self.hwnd, &mut pt);
        }
        let (x, y) = (pt.x as f32 / self.scale, pt.y as f32 / self.scale);
        if !self.hover_zone().contains(x, y) {
            self.tracking = false;
            self.input.mouse = None;
            self.set_hover(false);
        }
        self.broadcast(SystemEvent::DragOutDone);
        self.request_frame();
    }

    fn lp_to_logical(&self, lp: LPARAM) -> (f32, f32) {
        let x = (lp.0 & 0xFFFF) as u16 as i16 as f32;
        let y = ((lp.0 >> 16) & 0xFFFF) as u16 as i16 as f32;
        (x / self.scale, y / self.scale)
    }

    fn drain_bus(&mut self) {
        while let Ok(m) = self.rx.try_recv() {
            match m {
                Msg::Module(id, payload) => {
                    if let Some(i) = self.modules.iter().position(|m| m.id() == id) {
                        if self.started[i] {
                            let mut fx = Effects::default();
                            {
                                let mut cx = Cx {
                                    bus: &self.bus,
                                    cfg: &self.cfg,
                                    fx: &mut fx,
                                    hwnd: SendHwnd::new(self.hwnd),
                                    now: Instant::now(),
                                };
                                self.modules[i].on_message(payload, &mut cx);
                            }
                            fx.redraw = true;
                            self.apply_effects(fx);
                        }
                    }
                }
                Msg::ConfigFileChanged => self.reload_config(),
            }
        }
        self.schedule_wake();
    }

    fn broadcast(&mut self, ev: SystemEvent) {
        let mut fx = Effects::default();
        {
            let now = Instant::now();
            let hwnd = SendHwnd::new(self.hwnd);
            for (i, m) in self.modules.iter_mut().enumerate() {
                if self.started[i] {
                    let mut cx = Cx { bus: &self.bus, cfg: &self.cfg, fx: &mut fx, hwnd, now };
                    m.on_system(&ev, &mut cx);
                }
            }
        }
        self.apply_effects(fx);
        self.schedule_wake();
    }

    fn apply_effects(&mut self, fx: Effects) {
        for (s, k, v) in &fx.save_config {
            config::set_value(s, k, v);
        }
        for p in fx.peeks {
            self.push_peek(p);
        }
        if let Some(page) = fx.open_page {
            self.expand(Some(page));
        }
        if fx.expand && self.mode != Mode::Expanded {
            self.expand(None);
        }
        if fx.collapse {
            self.collapse();
        }
        if let Some(k) = fx.keyboard_focus {
            self.set_keyboard(k);
        }
        if let Some(files) = fx.drag_files {
            if !files.is_empty() && self.pending_drag.is_none() {
                self.pending_drag = Some(files);
                unsafe {
                    let _ = PostMessageW(Some(self.hwnd), WM_APP_DRAG, WPARAM(0), LPARAM(0));
                }
            }
        }
        if fx.redraw || fx.animate || fx.layout_changed {
            self.request_frame();
        }
    }

    // -----------------------------------------------------------------------
    // Modules
    // -----------------------------------------------------------------------

    fn sync_modules(&mut self) {
        let hwnd = SendHwnd::new(self.hwnd);
        let mut fx = Effects::default();
        for (i, m) in self.modules.iter_mut().enumerate() {
            let want = self.cfg.modules.is_enabled(m.id());
            if want && !self.started[i] {
                let mut cx = Cx { bus: &self.bus, cfg: &self.cfg, fx: &mut fx, hwnd, now: Instant::now() };
                m.start(&mut cx);
                self.started[i] = true;
            } else if !want && self.started[i] {
                m.stop();
                self.started[i] = false;
            }
        }
        if !self.page_available(self.page) {
            self.page = HOME;
        }
        self.apply_effects(fx);
        self.request_frame();
    }

    fn page_available(&self, page: ModuleId) -> bool {
        page == HOME || self.modules.iter().enumerate().any(|(i, m)| self.started[i] && m.id() == page && m.has_page())
    }

    fn module_index(&self, id: ModuleId) -> Option<usize> {
        self.modules.iter().position(|m| m.id() == id)
    }

    fn refresh_activity(&mut self) {
        let mut acts: Vec<Activity> = Vec::new();
        self.indicators.clear();
        for (i, m) in self.modules.iter().enumerate() {
            if !self.started[i] {
                continue;
            }
            if let Some(a) = m.activity() {
                acts.push(a);
            }
            m.indicators(&mut self.indicators);
        }
        // stable sort: equal priorities keep module order
        acts.sort_by_key(|a| std::cmp::Reverse(a.priority));
        let mut it = acts.into_iter();
        self.activity = it.next();
        self.secondary = it.next();
        // accent from album art if configured
        self.accent = match Color::parse(&self.cfg.appearance.accent) {
            Some(c) => c,
            None => self
                .module_index("media")
                .filter(|&i| self.started[i])
                .and_then(|i| self.modules[i].activity())
                .and_then(|a| match a.left {
                    Slot::Image(img) => img.accent,
                    _ => None,
                })
                .unwrap_or(Color::white(0.92)),
        };
    }

    fn on_wake(&mut self) {
        let now = Instant::now();
        // module ticks
        let mut fx = Effects::default();
        {
            let hwnd = SendHwnd::new(self.hwnd);
            for (i, m) in self.modules.iter_mut().enumerate() {
                if self.started[i] && self.tick_at[i].is_some_and(|t| t <= now + Duration::from_millis(2)) {
                    let mut cx = Cx { bus: &self.bus, cfg: &self.cfg, fx: &mut fx, hwnd, now };
                    m.on_tick(&mut cx);
                    fx.redraw = true;
                }
            }
        }
        self.apply_effects(fx);
        self.process_timeouts(now);
        if self.trim_at.is_some_and(|t| t <= now + Duration::from_millis(2)) {
            self.trim_at = None;
            self.res.trim_bitmaps();
            self.host.reset_dc(&self.gfx);
            self.gfx.trim();
        }
        if self.mode == Mode::Expanded && self.page == HOME {
            let st = unsafe { GetLocalTime() };
            if st.wMinute != self.clock_min {
                self.request_frame();
            }
        }
        self.schedule_wake();
    }

    /// Arm a single Win32 timer for the earliest pending deadline.
    fn schedule_wake(&mut self) {
        let now = Instant::now();
        let mut next: Option<Instant> = None;
        let mut consider = |t: Instant| {
            next = Some(next.map_or(t, |n: Instant| n.min(t)));
        };
        for (i, m) in self.modules.iter().enumerate() {
            self.tick_at[i] = if self.started[i] { m.next_tick() } else { None };
            if let Some(t) = self.tick_at[i] {
                consider(t);
            }
        }
        let g = &self.cfg.general;
        if self.mode != Mode::Expanded && self.hovering && g.expand_on_hover {
            if let Some(h) = self.hover_since {
                consider(h + Duration::from_millis(g.hover_delay_ms as u64));
            }
        }
        if self.mode == Mode::Expanded && !self.hovering && !self.keyboard && !self.drag_hover {
            if let Some(l) = self.leave_since {
                consider(l + Duration::from_millis(g.collapse_delay_ms as u64));
            }
            if let Some(t) = self.auto_close {
                consider(t);
            }
        }
        if let Some(p) = &self.peek {
            consider(p.until);
        }
        if let Some(t) = self.trim_at {
            consider(t);
        }
        if self.mode == Mode::Expanded && self.page == HOME {
            consider(now + Duration::from_secs(15));
        }
        unsafe {
            match next {
                Some(t) => {
                    let ms = t.saturating_duration_since(now).as_millis().clamp(1, 0x7FFF_FFFF) as u32;
                    SetTimer(Some(self.hwnd), TIMER_WAKE, ms, None);
                }
                None => {
                    let _ = KillTimer(Some(self.hwnd), TIMER_WAKE);
                }
            }
        }
    }

    fn process_timeouts(&mut self, now: Instant) {
        let g = self.cfg.general.clone();
        if self.mode != Mode::Expanded && self.hovering && g.expand_on_hover && !self.user_hidden {
            if let Some(h) = self.hover_since {
                if now >= h + Duration::from_millis(g.hover_delay_ms as u64) {
                    self.hover_since = None;
                    let page = self.peek.as_ref().and_then(|p| p.peek.page);
                    self.expand(page);
                }
            }
        }
        if self.mode == Mode::Expanded && !self.hovering && !self.keyboard && !self.drag_hover {
            let leave = self.leave_since.map(|l| l + Duration::from_millis(g.collapse_delay_ms as u64));
            if leave.into_iter().chain(self.auto_close).any(|t| now >= t) {
                self.collapse();
            }
        }
        if let Some(p) = &self.peek {
            if now >= p.until && !(self.hovering && self.mode == Mode::Peek) {
                self.peek = None;
                self.next_peek();
            }
        }
    }

    // -----------------------------------------------------------------------
    // State transitions
    // -----------------------------------------------------------------------

    fn expand(&mut self, page: Option<ModuleId>) {
        let target = page.filter(|p| self.page_available(p)).unwrap_or_else(|| {
            if self.mode == Mode::Expanded {
                self.page
            } else {
                self.activity.as_ref().and_then(|a| a.page).filter(|p| self.page_available(p)).unwrap_or(HOME)
            }
        });
        let was = self.mode;
        self.page = target;
        self.mode = Mode::Expanded;
        self.peek = None;
        self.leave_since = None;
        self.auto_close = if self.hovering { None } else { Some(Instant::now() + Duration::from_secs(8)) };
        if was != Mode::Expanded {
            self.broadcast(SystemEvent::Expanded(true));
        }
        self.request_frame();
        self.schedule_wake();
    }

    fn collapse(&mut self) {
        if self.mode != Mode::Expanded {
            return;
        }
        self.mode = Mode::Idle;
        self.leave_since = None;
        self.auto_close = None;
        self.hover_since = None;
        if self.keyboard {
            self.set_keyboard(false);
        }
        self.broadcast(SystemEvent::Expanded(false));
        self.next_peek();
        self.request_frame();
        self.schedule_wake();
    }

    fn toggle(&mut self) {
        if self.mode == Mode::Expanded {
            self.collapse();
        } else {
            if self.user_hidden {
                self.user_hidden = false;
            }
            self.expand(None);
        }
    }

    fn push_peek(&mut self, p: Peek) {
        if self.user_hidden || self.fs_hidden {
            return;
        }
        if self.mode == Mode::Expanded {
            return; // the user is already looking at the notch
        }
        if let (Some(k), Some(cur)) = (p.key, self.peek.as_mut()) {
            if cur.peek.key == Some(k) {
                cur.until = Instant::now() + p.duration;
                cur.peek = p;
                self.request_frame();
                self.schedule_wake();
                return;
            }
        }
        if let Some(k) = p.key {
            self.peek_queue.retain(|q| q.key != Some(k));
        }
        if p.instant {
            if let Some(cur) = self.peek.take() {
                self.peek_queue.push_front(cur.peek);
                self.peek_queue.truncate(4);
            }
        }
        if self.peek.is_some() {
            if self.peek_queue.len() >= 4 {
                self.peek_queue.pop_front();
            }
            self.peek_queue.push_back(p);
        } else {
            self.peek = Some(ActivePeek { until: Instant::now() + p.duration, peek: p });
            self.mode = Mode::Peek;
        }
        self.request_frame();
        self.schedule_wake();
    }

    fn next_peek(&mut self) {
        if self.mode == Mode::Expanded {
            return;
        }
        match self.peek_queue.pop_front() {
            Some(p) => {
                self.peek = Some(ActivePeek { until: Instant::now() + p.duration, peek: p });
                self.mode = Mode::Peek;
            }
            None => {
                self.peek = None;
                self.mode = Mode::Idle;
            }
        }
        self.request_frame();
    }

    fn set_hover(&mut self, on: bool) {
        if on == self.hovering {
            return;
        }
        self.hovering = on;
        let now = Instant::now();
        if on {
            self.hover_since = Some(now);
            self.leave_since = None;
            self.auto_close = None;
        } else {
            self.hover_since = None;
            self.leave_since = Some(now);
        }
        self.request_frame();
        self.schedule_wake();
    }

    fn set_keyboard(&mut self, on: bool) {
        if on == self.keyboard {
            return;
        }
        self.keyboard = on;
        unsafe {
            let ex = GetWindowLongPtrW(self.hwnd, GWL_EXSTYLE);
            if on {
                SetWindowLongPtrW(self.hwnd, GWL_EXSTYLE, ex & !(WS_EX_NOACTIVATE.0 as isize));
                if !SetForegroundWindow(self.hwnd).as_bool() || GetForegroundWindow() != self.hwnd {
                    // Unlock foreground rights with a harmless synthetic key press.
                    let inputs = [key_input(VK_MENU, false), key_input(VK_MENU, true)];
                    SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
                    let _ = SetForegroundWindow(self.hwnd);
                }
                let _ = SetFocus(Some(self.hwnd));
            } else {
                SetWindowLongPtrW(self.hwnd, GWL_EXSTYLE, ex | WS_EX_NOACTIVATE.0 as isize);
            }
        }
        self.request_frame();
    }

    fn quit(&mut self) {
        self.shutdown();
        unsafe {
            let _ = DestroyWindow(self.hwnd);
            // WM_DESTROY arrives while the app is still borrowed (re-entrant),
            // so its handler never runs; end the message loop here.
            PostQuitMessage(0);
        }
    }

    // -----------------------------------------------------------------------
    // Input
    // -----------------------------------------------------------------------

    fn body_rect(&self) -> Rect {
        let w = self.anim.w.value;
        let h = self.anim.h.value;
        let shift = (1.0 - self.anim.shown.value) * (h + 14.0);
        Rect::new(self.win_w * 0.5 - w * 0.5, -shift, w, h)
    }

    /// The secondary activity bubble slides out from under the right edge.
    fn bubble_rect(&self) -> Rect {
        let s = self.cfg.appearance.notch_height;
        let b = self.body_rect();
        let k = self.anim.sec.value;
        Rect::new(b.right() - s + (s + 8.0) * k, b.y.min(0.0), s, s)
    }

    fn hover_zone(&self) -> Rect {
        let b = self.body_rect();
        let strip_w = self.cfg.appearance.notch_width;
        let strip = Rect::new(self.win_w * 0.5 - strip_w * 0.5, 0.0, strip_w, 5.0);
        let b = Rect::new(b.x, 0.0, b.w, b.bottom().max(0.0));
        let z = b.union(&strip);
        if self.anim.sec.value > 0.5 {
            z.union(&self.bubble_rect())
        } else {
            z
        }
    }

    fn on_mouse_move(&mut self, x: f32, y: f32) {
        if !self.tracking {
            let mut tme = TRACKMOUSEEVENT {
                cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                dwFlags: TME_LEAVE,
                hwndTrack: self.hwnd,
                dwHoverTime: 0,
            };
            unsafe {
                let _ = TrackMouseEvent(&mut tme);
            }
            self.tracking = true;
        }
        let inside = self.hover_zone().contains(x, y) && !self.fs_hidden;
        self.set_hover(inside);
        let before = self.input.mouse;
        self.input.mouse = if inside { Some((x, y)) } else { None };
        // Only the expanded/peek UI has hover states worth a frame.
        if self.mode != Mode::Idle && before != self.input.mouse {
            self.request_frame();
        }
    }

    fn on_click(&mut self, p: (f32, f32)) {
        let down = self.input.down.take();
        let inside = self.body_rect().contains(p.0, p.1);
        match self.mode {
            Mode::Expanded => {
                if !inside {
                    self.collapse();
                    return;
                }
                if let Some(d) = down {
                    self.input.click = Some((d, p));
                }
            }
            Mode::Peek => match self.peek.as_ref().and_then(|a| a.peek.focus) {
                Some(h) => {
                    sys::activate_window(h);
                    self.peek = None;
                    self.next_peek();
                }
                None => {
                    let page = self.peek.as_ref().and_then(|a| a.peek.page);
                    self.expand(page);
                }
            },
            Mode::Idle => {
                if self.anim.sec.value > 0.5 && self.bubble_rect().contains(p.0, p.1) {
                    let page = self.secondary.as_ref().and_then(|a| a.page);
                    self.expand(page);
                } else if inside || self.hover_zone().contains(p.0, p.1) {
                    self.user_hidden = false;
                    self.expand(None);
                }
            }
        }
        self.request_frame();
    }

    fn on_key(&mut self, k: Key) {
        if k == Key::Escape {
            self.set_keyboard(false);
            self.collapse();
            return;
        }
        let Some(i) = self.module_index(self.page) else { return };
        if !self.started[i] || !self.modules[i].wants_keyboard() {
            return;
        }
        let mut fx = Effects::default();
        {
            let mut cx =
                Cx { bus: &self.bus, cfg: &self.cfg, fx: &mut fx, hwnd: SendHwnd::new(self.hwnd), now: Instant::now() };
            self.modules[i].on_key(k, &mut cx);
        }
        fx.redraw = true;
        self.apply_effects(fx);
        self.schedule_wake();
    }

    fn on_hotkey(&mut self, a: Action) {
        match a {
            Action::Toggle => self.toggle(),
            Action::Ai => {
                if self.mode == Mode::Expanded && self.page == "ai" {
                    self.collapse();
                } else {
                    self.user_hidden = false;
                    self.expand(Some("ai"));
                    if self.page == "ai" {
                        self.set_keyboard(true);
                    }
                }
            }
            Action::Timer => self.expand(Some("timer")),
            Action::Clipboard => self.expand(Some("clipboard")),
            Action::PlayPause => self.broadcast(SystemEvent::Command { verb: "playpause".into(), args: vec![] }),
            Action::Dictate => {
                self.broadcast(SystemEvent::Command { verb: "dictate".into(), args: vec!["hotkey".into()] })
            }
        }
    }

    pub fn on_ipc(&mut self, args: Vec<String>) {
        let Some(verb) = args.first().map(|s| s.to_ascii_lowercase()) else {
            self.expand(None);
            return;
        };
        let rest: Vec<String> = args[1..].to_vec();
        log!("ipc: {verb} {rest:?}");
        match verb.as_str() {
            "toggle" => self.toggle(),
            "expand" | "show" | "open" => {
                self.user_hidden = false;
                self.expand(rest.first().map(|p| leak_page(p)));
            }
            "collapse" | "close" => self.collapse(),
            "ai" => self.on_hotkey(Action::Ai),
            "reload" => self.reload_config(),
            "notify" => {
                // Scriptable banner: dynamic-notch notify "Title" "Body"
                let title = rest.first().cloned().unwrap_or_else(|| "Notification".into());
                let body = rest.get(1..).map(|a| a.join(" ")).unwrap_or_default();
                self.push_peek(Peek::new(Icon::Bell, palette::BLUE, title, body).duration_ms(4500));
            }
            "claude" => self.push_peek(claude_peek(&rest)),
            "quit" | "exit" => self.quit(),
            _ => self.broadcast(SystemEvent::Command { verb, args: rest }),
        }
    }

    // -----------------------------------------------------------------------
    // Tray menu
    // -----------------------------------------------------------------------

    pub fn menu_state(&self) -> MenuState {
        MenuState {
            expanded: self.mode == Mode::Expanded,
            user_hidden: self.user_hidden,
            modules: self
                .modules
                .iter()
                .map(|m| (m.title().to_string(), self.cfg.modules.is_enabled(m.id())))
                .collect(),
            monitor_count: monitors().len(),
            monitor: self.cfg.general.monitor.clone(),
            autostart: self.cfg.general.start_with_windows,
            blur: self.cfg.appearance.blur,
            hover: self.cfg.general.expand_on_hover,
            hide_fullscreen: self.cfg.general.hide_in_fullscreen,
        }
    }

    pub fn on_menu(&mut self, cmd: u32) {
        let b = |v: bool| if v { "true".to_string() } else { "false".to_string() };
        match cmd {
            tray::CMD_TOGGLE => self.toggle(),
            tray::CMD_AI => self.on_hotkey(Action::Ai),
            tray::CMD_TIMER => self.expand(Some("timer")),
            tray::CMD_HIDE => {
                self.user_hidden = !self.user_hidden;
                if self.user_hidden {
                    self.collapse();
                    self.peek = None;
                    self.peek_queue.clear();
                    self.mode = Mode::Idle;
                }
                self.request_frame();
            }
            tray::CMD_MON_PRIMARY => config::set_value("general", "monitor", "\"primary\""),
            tray::CMD_MON_ACTIVE => config::set_value("general", "monitor", "\"active\""),
            c if (tray::CMD_MON_BASE..tray::CMD_MON_BASE + 16).contains(&c) => {
                config::set_value("general", "monitor", &format!("\"{}\"", c - tray::CMD_MON_BASE + 1))
            }
            c if (tray::CMD_MODULE_BASE..tray::CMD_MODULE_BASE + 32).contains(&c) => {
                let i = (c - tray::CMD_MODULE_BASE) as usize;
                if let Some(m) = self.modules.get(i) {
                    let id = m.id();
                    let on = !self.cfg.modules.is_enabled(id);
                    config::set_value("modules", id, &b(on));
                }
            }
            tray::CMD_AUTOSTART => {
                config::set_value("general", "start_with_windows", &b(!self.cfg.general.start_with_windows))
            }
            tray::CMD_BLUR => config::set_value("appearance", "blur", &b(!self.cfg.appearance.blur)),
            tray::CMD_HOVER => config::set_value("general", "expand_on_hover", &b(!self.cfg.general.expand_on_hover)),
            tray::CMD_FULLSCREEN => {
                config::set_value("general", "hide_in_fullscreen", &b(!self.cfg.general.hide_in_fullscreen))
            }
            tray::CMD_EDIT_CONFIG => {
                let path = config::config_path();
                if !path.exists() {
                    let _ = std::fs::write(&path, config::DEFAULT_TEMPLATE);
                }
                sys::shell_open(&path.to_string_lossy());
            }
            tray::CMD_OPEN_FOLDER => sys::shell_open(&crate::util::app_dir().to_string_lossy()),
            tray::CMD_RELOAD => self.reload_config(),
            tray::CMD_QUIT => self.quit(),
            _ => {}
        }
    }

    fn reload_config(&mut self) {
        match config::load() {
            LoadResult::Ok(c) => {
                let autostart_changed = c.general.start_with_windows != self.cfg.general.start_with_windows;
                self.cfg = c;
                if autostart_changed {
                    sys::set_autostart(self.cfg.general.start_with_windows);
                }
                self.register_hotkeys();
                self.sync_modules();
                self.place();
                self.broadcast(SystemEvent::ConfigReloaded);
                log!("config reloaded");
            }
            LoadResult::Invalid(e) => {
                log!("config error: {e}");
                let msg = crate::util::one_line(&e);
                self.push_peek(
                    Peek::new(Icon::Settings, palette::ORANGE, "Settings file has an error", msg)
                        .duration_ms(6000)
                        .key("config"),
                );
            }
        }
    }

    fn register_hotkeys(&mut self) {
        let (ok, errors) = hotkeys::register_all(self.hwnd, &self.cfg.hotkeys);
        self.hotkeys = ok;
        self.dictate_vk = hotkeys::parse(&self.cfg.hotkeys.dictate).map_or(0, |(_, vk)| vk as u16);
        if !errors.is_empty() {
            log!("hotkey problems: {errors:?}");
            self.push_peek(
                Peek::new(Icon::Settings, palette::ORANGE, "Hotkey unavailable", errors.join(", "))
                    .duration_ms(5000)
                    .key("hotkeys"),
            );
        }
    }

    // -----------------------------------------------------------------------
    // Placement (multi-monitor + per-monitor DPI)
    // -----------------------------------------------------------------------

    fn place(&mut self) {
        let mon = self.pick_monitor();
        unsafe {
            let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
            if !GetMonitorInfoW(mon, &mut mi).as_bool() {
                return;
            }
            let (mut dx, mut dy) = (96u32, 96u32);
            let _ = GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
            let scale = dx as f32 / 96.0 * self.cfg.appearance.scale;
            let win_w = self.cfg.appearance.expanded_width + 2.0 * MARGIN;
            let win_h = WIN_H;
            let wpx = (win_w * scale).ceil() as i32;
            let hpx = (win_h * scale).ceil() as i32;
            let m = mi.rcMonitor;
            let top = if mi.rcWork.top > m.top { mi.rcWork.top } else { m.top };
            let x = (m.left + m.right) / 2 - wpx / 2;
            let geom = (x, top, wpx, hpx);
            self.monitor = mon;
            if geom != self.win_px || (scale - self.scale).abs() > 0.001 {
                self.scale = scale;
                self.win_w = win_w;
                self.win_h = win_h;
                self.win_px = geom;
                let _ = SetWindowPos(self.hwnd, Some(HWND_TOPMOST), x, top, wpx, hpx, SWP_NOACTIVATE);
                if let Err(e) = self.host.resize(wpx, hpx) {
                    log!("surface resize failed: {e}");
                }
                self.region = None;
                self.region_anim = None;
            }
        }
        self.check_fullscreen();
        self.request_frame();
    }

    fn pick_monitor(&self) -> HMONITOR {
        let primary = unsafe { MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY) };
        match self.cfg.general.monitor.trim() {
            "active" => unsafe {
                let fg = GetForegroundWindow();
                if fg.is_invalid() || fg == self.hwnd {
                    if self.monitor.is_invalid() {
                        primary
                    } else {
                        self.monitor
                    }
                } else {
                    MonitorFromWindow(fg, MONITOR_DEFAULTTOPRIMARY)
                }
            },
            s => match s.parse::<usize>() {
                Ok(n) if n >= 1 => monitors().get(n - 1).copied().unwrap_or(primary),
                _ => primary,
            },
        }
    }

    fn on_foreground_changed(&mut self) {
        unsafe {
            let fg = GetForegroundWindow();
            // Re-hook location changes for the foreground process only, so a
            // window becoming fullscreen (F11, videos) is noticed without
            // listening to every window in the system.
            let mut pid = 0u32;
            GetWindowThreadProcessId(fg, Some(&mut pid));
            if pid != self.loc_pid && pid != std::process::id() {
                if let Some(h) = self.loc_hook.take() {
                    let _ = UnhookWinEvent(h);
                }
                let h = SetWinEventHook(
                    EVENT_OBJECT_LOCATIONCHANGE,
                    EVENT_OBJECT_LOCATIONCHANGE,
                    None,
                    Some(win_event_cb),
                    pid,
                    0,
                    WINEVENT_OUTOFCONTEXT,
                );
                if !h.is_invalid() {
                    self.loc_hook = Some(h);
                }
                self.loc_pid = pid;
            }
        }
        if self.cfg.general.monitor == "active" {
            let m = self.pick_monitor();
            if m != self.monitor {
                self.place();
                return;
            }
        }
        self.check_fullscreen();
        if !self.fs_hidden {
            unsafe {
                let _ =
                    SetWindowPos(self.hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
            }
        }
    }

    fn check_fullscreen(&mut self) {
        let hide = self.cfg.general.hide_in_fullscreen && unsafe { is_fullscreen_on(self.monitor, self.hwnd) };
        if hide != self.fs_hidden {
            self.fs_hidden = hide;
            if hide {
                self.collapse();
            }
            log!("fullscreen hidden = {hide}");
            self.request_frame();
        }
    }

    // -----------------------------------------------------------------------
    // Frame
    // -----------------------------------------------------------------------

    fn request_frame(&mut self) {
        self.dirty = true;
        self.pacer.start();
    }

    fn targets(&self) -> Targets {
        let ap = &self.cfg.appearance;
        let base_w = ap.notch_width;
        let base_h = ap.notch_height;
        let mut t = Targets {
            w: base_w,
            h: base_h,
            r: (base_h * 0.36).min(13.0),
            ear: (base_h * 0.22).clamp(4.0, 8.0),
            glass: 0.0,
            compact: false,
            peek: false,
            exp: false,
            shown: !(self.user_hidden || self.fs_hidden),
            sec: false,
        };
        match self.mode {
            Mode::Expanded => {
                let content_w = ap.expanded_width - 2.0 * PAD;
                let content_h = if self.page == HOME {
                    let tiles = (0..self.modules.len())
                        .filter(|&i| self.started[i] && self.modules[i].card() == Some(CardSize::Small))
                        .count();
                    if tiles >= HOME_TILES {
                        HOME_HEIGHT_TALL
                    } else {
                        HOME_HEIGHT
                    }
                } else {
                    self.module_index(self.page)
                        .map(|i| self.modules[i].page_height(content_w))
                        .unwrap_or(HOME_HEIGHT)
                        .clamp(60.0, WIN_H - MARGIN - HEADER_H - BOTTOM_PAD)
                };
                t.w = ap.expanded_width;
                t.h = HEADER_H + content_h + BOTTOM_PAD;
                t.r = 34.0;
                t.ear = 12.0;
                t.glass = 1.0;
                t.exp = true;
            }
            Mode::Peek => {
                t.w = (base_w + 150.0).max(360.0).min(ap.expanded_width);
                t.h = base_h + 46.0;
                t.r = 26.0;
                t.ear = 10.0;
                t.glass = 0.55;
                t.peek = true;
            }
            Mode::Idle => {
                if let Some(a) = &self.activity {
                    t.w = base_w + 2.0 * a.wing * base_h;
                    t.compact = true;
                    t.sec = self.secondary.is_some() && t.shown;
                } else {
                    match self.cfg.general.idle_style.as_str() {
                        "pill" if !self.hovering => {
                            t.w = base_w * 0.55;
                            t.h = 7.0;
                            t.r = 3.5;
                            t.ear = 3.0;
                        }
                        "hidden" if !self.hovering => t.shown = false,
                        _ => {}
                    }
                }
                if self.hovering && !self.user_hidden {
                    t.w += 14.0;
                    t.h += 4.0;
                    t.r += 1.5;
                }
            }
        }
        t
    }

    fn retarget(&mut self) {
        let t = self.targets();
        let ap = &self.cfg.appearance;
        let speed = ap.animation_speed;
        let bounce = ap.bounce;
        let a = &mut self.anim;
        let grow = t.w > a.w.value + 0.5 || t.h > a.h.value + 0.5;
        let (resp, damp) = if grow { (0.5 / speed, 1.0 - 0.6 * bounce) } else { (0.3 / speed, 1.0 - 0.15 * bounce) };
        for s in [&mut a.w, &mut a.h, &mut a.r] {
            s.set_params(resp, damp);
        }
        a.w.set_target(t.w);
        a.h.set_target(t.h);
        a.r.set_target(t.r);
        a.ear.set_target(t.ear);
        a.glass.set_target(t.glass);
        a.shown.set_params(0.5 / speed, 1.0 - 0.5 * bounce);
        a.shown.set_target(if t.shown { 1.0 } else { 0.0 });
        a.sec.set_params(0.45 / speed, 1.0 - 0.6 * bounce);
        a.sec.set_target(if t.sec { 1.0 } else { 0.0 });
        let fade = |s: &mut Spring, on: bool, delay: f32| {
            if on {
                s.set_params(0.24 / speed, 1.0);
                s.set_target_delayed(1.0, delay / speed);
            } else {
                s.set_params(0.12 / speed, 1.0);
                s.set_target(0.0);
            }
        };
        fade(&mut a.compact, t.compact, 0.05);
        fade(&mut a.peek, t.peek, 0.08);
        fade(&mut a.exp, t.exp, 0.09);
    }

    fn frame(&mut self) {
        let now = Instant::now();
        let dt = (now - self.last_frame).as_secs_f32().min(1.0 / 20.0);
        self.last_frame = now;
        self.dirty = false;
        self.trim_at = None;
        let shown = !(self.user_hidden || self.fs_hidden) && self.mode != Mode::Expanded;
        if PEEKS_SHOWN.swap(shown, Ordering::Relaxed) != shown {
            self.broadcast(SystemEvent::PeeksShown(shown));
        }
        self.process_timeouts(now);
        self.refresh_activity();
        self.retarget();
        let moving = self.anim.step(dt);
        let fx = self.render(now);
        let animate = fx.animate;
        self.apply_effects(fx);
        self.update_region(!moving);
        self.input.click = None;
        self.input.right_click = None;
        self.input.wheel = 0.0;
        if self.debug {
            let st = &mut self.stats;
            st.0.get_or_insert(now);
            if moving {
                st.1 += 1;
            } else if animate {
                st.2 += 1;
            } else if self.dirty {
                st.3 += 1;
            }
        }
        if !moving && !animate && !self.dirty {
            if let (true, Some(t0)) = (self.debug, self.stats.0) {
                let (_, a, b, c) = self.stats;
                log!(
                    "burst: {} frames in {} ms (spring {a}, animate {b}, dirty {c}) mode {:?}",
                    a + b + c + 1,
                    t0.elapsed().as_millis(),
                    self.mode
                );
                self.stats = (None, 0, 0, 0);
            }
            self.pacer.stop();
            // Release GPU scratch memory once nothing has been drawn for a
            // while. Doing it after every burst would rebuild the glyph and
            // image caches on each once-a-second update (stats, timer, media).
            self.trim_at = Some(now + TRIM_DELAY);
        }
        if !self.shown_once {
            self.shown_once = true;
        }
        self.schedule_wake();
    }

    fn update_region(&mut self, settled: bool) {
        let ear = self.anim.ear.target.max(self.anim.ear.value);
        let t = self.targets();
        let shown = t.shown;
        let bubble = if t.sec { self.cfg.appearance.notch_height + 8.0 + ear } else { 0.0 };
        let tb = Rect::new(self.win_w * 0.5 - t.w * 0.5 - ear, 0.0, t.w + 2.0 * ear + bubble, t.h);
        let cur = self.body_rect();
        let mut cb = Rect::new(cur.x - ear, 0.0, cur.w + 2.0 * ear, cur.bottom().max(0.0));
        if self.anim.sec.value > 0.01 {
            cb = cb.union(&self.bubble_rect().expand(ear));
        }
        let strip_w = self.cfg.appearance.notch_width;
        let strip = Rect::new(self.win_w * 0.5 - strip_w * 0.5, 0.0, strip_w, 5.0);
        let want = if self.fs_hidden {
            None
        } else if settled {
            self.region_anim = None;
            Some(if shown { tb.union(&strip) } else { strip })
        } else {
            let mut r = tb.expand(16.0).union(&cb).union(&strip);
            if let Some(prev) = self.region_anim {
                r = r.union(&prev);
            }
            self.region_anim = Some(r);
            Some(r)
        };
        let s = self.scale;
        let px = match want {
            Some(r) => (
                (r.x * s).floor().max(0.0) as i32,
                0,
                (r.right() * s).ceil() as i32,
                ((r.bottom() * s).ceil() as i32).min(self.win_px.3),
            ),
            None => (0, 0, 0, 0),
        };
        if self.region != Some(px) {
            self.region = Some(px);
            unsafe {
                let rgn = CreateRectRgn(px.0, px.1, px.2, px.3);
                SetWindowRgn(self.hwnd, Some(rgn), false);
            }
        }
    }

    fn render(&mut self, now: Instant) -> Effects {
        let mut fx = Effects::default();
        let (dc, off) = match self.host.begin_draw() {
            Ok(v) => v,
            Err(e) => {
                log!("BeginDraw failed: {e} — recreating devices");
                self.recover_device();
                return fx;
            }
        };
        let body = self.body_rect();
        let scale = self.scale;
        let ap = self.cfg.appearance.clone();
        let a_glass = self.anim.glass.value;
        let a_r = self.anim.r.value;
        let a_ear = self.anim.ear.value;
        let a_compact = self.anim.compact.value;
        let a_peek = self.anim.peek.value;
        let a_exp = self.anim.exp.value;
        let target_h = self.targets().h;
        let bubble = self.bubble_rect();
        let secondary = self.secondary.clone();
        let mut bars: Option<(Rect, Color, bool)> = None;
        let tabs: Vec<(ModuleId, Icon, String)> = std::iter::once((HOME, Icon::Home, "Home".to_string()))
            .chain(
                self.modules
                    .iter()
                    .enumerate()
                    .filter(|(i, m)| self.started[*i] && m.has_page())
                    .map(|(_, m)| (m.id(), m.icon(), m.title().to_string())),
            )
            .collect();

        {
            let mut p = Painter::new(dc.clone(), &self.gfx, &mut self.res, scale, off.x as f32, off.y as f32);
            p.clear();
            if body.bottom() > 0.25 {
                // shadow
                if ap.shadow && a_glass > 0.01 {
                    // extend above the window so only the sides/bottom show
                    let sr = Rect::new(body.x, body.y - 60.0, body.w, body.h + 60.0);
                    p.shadow(sr, a_r, 0.5 * a_glass);
                }
                // secondary activity bubble (drawn first; the main body covers it while sliding out)
                let sec_v = self.anim.sec.value;
                if sec_v > 0.01 {
                    if let Some(sa) = secondary {
                        let br = bubble;
                        if let Some(g) = gfx::notch_path(&p, br, br.w * 0.5, a_ear.min(6.0)) {
                            p.fill_geometry(&g, Color::black(ap.collapsed_opacity));
                        }
                        let input = self.input.clone();
                        let mut ui = Ui {
                            p: &mut p,
                            input: &input,
                            fx: &mut fx,
                            cfg: &self.cfg,
                            bus: &self.bus,
                            now,
                            accent: self.accent,
                            hot: false,
                            interactive: false,
                        };
                        ui.p.alpha = sec_v.clamp(0.0, 1.0);
                        let slot = br.inset(5.0);
                        let mut no_bars = None;
                        draw_slot(&mut ui, &sa.left, slot, slot, true, &mut no_bars);
                        ui.p.alpha = 1.0;
                    }
                }
                let opaque = if ap.blur { ap.expanded_opacity } else { ap.expanded_opacity.max(0.95) };
                let bg_alpha = lerp(ap.collapsed_opacity, opaque, a_glass);
                if let Some(g) = gfx::notch_path(&p, body, a_r, a_ear) {
                    p.fill_geometry(&g, Color::black(bg_alpha));
                }

                p.push_clip(body);
                let input = self.input.clone();
                let accent = self.accent;
                let mut ui = Ui {
                    p: &mut p,
                    input: &input,
                    fx: &mut fx,
                    cfg: &self.cfg,
                    bus: &self.bus,
                    now,
                    accent,
                    hot: false,
                    interactive: false,
                };

                // compact live activity / idle indicators
                if a_compact > 0.003 || self.mode == Mode::Idle {
                    ui.p.alpha = if self.activity.is_some() { a_compact } else { 1.0 - a_peek.max(a_exp) };
                    draw_compact(&mut ui, body, self.activity.as_ref(), &self.indicators, ap.notch_height, &mut bars);
                }
                // peek banner
                if a_peek > 0.003 {
                    if let Some(pk) = &self.peek {
                        ui.p.alpha = a_peek;
                        let s = 0.92 + 0.08 * a_peek;
                        ui.p.set_local(s, body.cx(), 0.0, 0.0, 0.0);
                        ui.interactive = a_peek > 0.9;
                        draw_peek(&mut ui, body, &pk.peek);
                        ui.p.reset_local();
                    }
                }
                // expanded
                if a_exp > 0.003 {
                    ui.p.alpha = a_exp;
                    let s = 0.94 + 0.06 * a_exp;
                    ui.p.set_local(s, body.cx(), 0.0, 0.0, 0.0);
                    ui.interactive = a_exp > 0.85 && self.mode == Mode::Expanded;
                    let tw = self.cfg.appearance.expanded_width;
                    let full = Rect::new(body.cx() - tw * 0.5, body.y, tw, target_h.max(body.h));
                    draw_expanded(&mut ui, full, &mut self.modules, &self.started, &tabs, self.page, &self.indicators);
                    ui.p.reset_local();
                }
                ui.p.alpha = 1.0;
                p.pop_clip();
            }
        }
        if let Err(e) = self.host.end_draw() {
            log!("EndDraw failed: {e}");
            self.recover_device();
        }
        self.res.end_frame();

        // compositor-driven visualizer
        let show_bars = self.cfg.media.visualizer && self.mode == Mode::Idle;
        match bars {
            Some((r, c, playing)) if show_bars && a_compact > 0.01 => {
                let s = self.scale;
                let pr = Rect::new(r.x * s, r.y * s, r.w * s, r.h * s);
                self.host.bars.update(pr, c, a_compact * self.anim.shown.value, playing);
            }
            _ => self.host.bars.hide(),
        }

        // blur behind the notch body
        let s = self.scale;
        let bp = Rect::new(body.x * s, body.y * s, body.w * s, body.h.max(0.0) * s);
        let blur = if ap.blur { a_glass } else { 0.0 };
        self.host.set_backdrop(bp, a_r * s, blur * self.anim.shown.value);

        if let Some(page) = fx.open_page.take() {
            if self.mode == Mode::Expanded && self.page_available(page) {
                if self.page != page {
                    self.page = page;
                    self.set_keyboard(page == "ai");
                }
            } else {
                fx.open_page = Some(page);
            }
        }
        if self.mode == Mode::Expanded && self.page == HOME {
            let st = unsafe { GetLocalTime() };
            self.clock_min = st.wMinute;
        }
        fx
    }

    fn recover_device(&mut self) {
        if self.gfx.recreate_devices().is_ok() {
            let _ = self.host.set_rendering_device(&self.gfx);
            if let Ok(r) = Res::new(&self.gfx) {
                self.res = r;
            }
        }
        self.request_frame();
    }
}

// ---------------------------------------------------------------------------
// Drawing helpers (free functions to keep borrows simple)
// ---------------------------------------------------------------------------

fn draw_compact(
    ui: &mut Ui,
    body: Rect,
    activity: Option<&Activity>,
    indicators: &[Indicator],
    base_h: f32,
    bars: &mut Option<(Rect, Color, bool)>,
) {
    let h = body.h.min(base_h + 6.0);
    let slot = (base_h - 10.0).max(10.0);
    let cy = body.y + h * 0.5;
    let mut right_edge = body.right() - 10.0;
    if let Some(a) = activity {
        let wing = a.wing * base_h;
        let lr = Rect::new(body.x + 9.0, cy - slot * 0.5, slot, slot);
        let rr = Rect::new(body.right() - 9.0 - slot, cy - slot * 0.5, slot, slot);
        let lw = Rect::new(body.x + 9.0, cy - slot * 0.5, wing - 6.0, slot);
        let rw = Rect::new(body.right() - 9.0 - (wing - 6.0), cy - slot * 0.5, wing - 6.0, slot);
        draw_slot(ui, &a.left, lr, lw, true, bars);
        draw_slot(ui, &a.right, rr, rw, false, bars);
        right_edge = body.right() - wing - 4.0;
    }
    // privacy dots (mic / camera)
    let mut x = right_edge;
    for ind in indicators.iter().filter(|i| i.dot).rev() {
        ui.p.fill_circle(x - 3.0, cy, 3.2, ind.color);
        x -= 10.0;
    }
}

fn draw_slot(ui: &mut Ui, slot: &Slot, sq: Rect, wing: Rect, left: bool, bars: &mut Option<(Rect, Color, bool)>) {
    match slot {
        Slot::Empty => {}
        Slot::Image(img) => ui.p.image(img, sq, sq.w * 0.26, 1.0),
        Slot::Icon(ic, c) => ui.p.icon(*ic, sq.inset(sq.w * 0.06), *c),
        Slot::Text(s, c) => {
            let st = TextStyle::new((sq.h * 0.62).clamp(11.0, 15.0), *c).semibold().display();
            let st = if left { st } else { st.right() };
            ui.p.text(s, wing, &st);
        }
        Slot::Bars { color, playing } => {
            let r = Rect::new(sq.x + sq.w * 0.12, sq.y + sq.h * 0.18, sq.w * 0.76, sq.h * 0.64);
            *bars = Some((r, *color, *playing));
        }
        Slot::Ring(progress, c) => {
            let cx = sq.cx();
            let cy = sq.cy();
            let rad = sq.w * 0.36;
            let w = (sq.w * 0.12).max(2.0);
            ui.p.stroke_circle(cx, cy, rad, w, Color::white(0.18));
            match progress {
                Some(f) => ui.p.arc(cx, cy, rad, 0.0, std::f32::consts::TAU * f.clamp(0.0, 1.0), w, *c),
                None => {
                    let t = crate::ui::epoch_secs() * 5.0;
                    ui.p.arc(cx, cy, rad, t, 1.6, w, *c);
                    ui.animate();
                }
            }
        }
        Slot::Battery { level, charging } => {
            let c = battery_color(*level, *charging);
            icons::battery(ui.p, sq.inset_xy(0.0, sq.h * 0.18), *level, *charging, c);
        }
        Slot::Wave { levels, color } => {
            let r = Rect::new(wing.x, wing.y + wing.h * 0.12, wing.w, wing.h * 0.76);
            modules::voice::draw_wave(ui, r, levels, *color);
        }
    }
}

pub fn battery_color(level: f32, charging: bool) -> Color {
    if charging {
        palette::GREEN
    } else if level <= 0.1 {
        palette::RED
    } else if level <= 0.2 {
        palette::YELLOW
    } else {
        Color::white(0.92)
    }
}

fn draw_peek(ui: &mut Ui, body: Rect, pk: &Peek) {
    let top = body.y + 10.0;
    let inner = Rect::new(body.x + 18.0, top + 6.0, body.w - 36.0, body.h - top - 16.0);
    let is = (inner.h).min(44.0);
    let ir = Rect::new(inner.x, inner.cy() - is * 0.5, is, is);
    match &pk.image {
        Some(img) => ui.p.image(img, ir, 10.0, 1.0),
        None => {
            ui.p.fill_rounded(ir, is * 0.5, pk.accent.with_a(0.2));
            ui.p.icon(pk.icon, ir.inset(is * 0.24), pk.accent);
        }
    }
    if ui.hovered(body) {
        ui.hot = true;
    }
    if let Trailing::Level { frac, color, text } = &pk.trailing {
        // title (+ dim subtitle) on top, a slider-like bar below, value on the right
        let tx = ir.right() + 12.0;
        let vs = TextStyle::new(15.0, palette::TEXT).semibold().display().right();
        let value = text.clone().unwrap_or_else(|| format!("{:.0}", frac.clamp(0.0, 1.0) * 100.0));
        let vw = ui.p.measure("100", &vs).0.max(ui.p.measure(&value, &vs).0) + 4.0;
        let ts = TextStyle::new(13.0, palette::TEXT).semibold();
        let (tw, _) = ui.p.measure(&pk.title, &ts);
        let col_w = inner.right() - tx;
        ui.p.text(&pk.title, Rect::new(tx, inner.cy() - 20.0, col_w, 18.0), &ts);
        if !pk.subtitle.is_empty() && tw + 30.0 < col_w {
            let sx = tx + tw + 8.0;
            ui.p.text(
                &pk.subtitle,
                Rect::new(sx, inner.cy() - 20.0, inner.right() - sx, 18.0),
                &TextStyle::new(12.5, palette::TEXT_DIM),
            );
        }
        let bar = Rect::new(tx, inner.cy() + 6.0, col_w - vw - 10.0, 6.0);
        ui.progress(bar, *frac, *color);
        ui.p.text(&value, Rect::new(inner.right() - vw, bar.cy() - 11.0, vw, 22.0), &vs.color(*color));
        return;
    }
    let trailing_w = match &pk.trailing {
        Trailing::None | Trailing::Level { .. } => 0.0,
        Trailing::Text(s, _) => ui.p.measure(s, &TextStyle::new(15.0, palette::TEXT).semibold()).0 + 8.0,
        Trailing::Ring(..) => 34.0,
        Trailing::Battery { .. } => 40.0,
    };
    let tx = ir.right() + 12.0;
    let tw = inner.right() - tx - trailing_w;
    let has_sub = !pk.subtitle.is_empty();
    let title_r =
        if has_sub { Rect::new(tx, inner.cy() - 19.0, tw, 20.0) } else { Rect::new(tx, inner.cy() - 10.0, tw, 20.0) };
    ui.p.text(&pk.title, title_r, &TextStyle::new(14.5, palette::TEXT).semibold());
    if has_sub {
        ui.p.text(&pk.subtitle, Rect::new(tx, inner.cy() + 1.0, tw, 18.0), &TextStyle::new(12.5, palette::TEXT_DIM));
    }
    let tr = Rect::new(inner.right() - trailing_w, inner.y, trailing_w, inner.h);
    match &pk.trailing {
        Trailing::None | Trailing::Level { .. } => {}
        Trailing::Text(s, c) => ui.p.text(s, tr, &TextStyle::new(15.0, *c).semibold().display().right()),
        Trailing::Ring(f, c) => {
            let (cx, cy) = (tr.right() - 14.0, tr.cy());
            ui.p.stroke_circle(cx, cy, 11.0, 3.0, Color::white(0.18));
            ui.p.arc(cx, cy, 11.0, 0.0, std::f32::consts::TAU * f.clamp(0.0, 1.0), 3.0, *c);
        }
        Trailing::Battery { level, charging } => {
            let r = Rect::new(tr.right() - 36.0, tr.cy() - 8.0, 36.0, 16.0);
            icons::battery(ui.p, r, *level, *charging, battery_color(*level, *charging));
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_expanded(
    ui: &mut Ui,
    full: Rect,
    modules: &mut [Box<dyn Module>],
    started: &[bool],
    tabs: &[(ModuleId, Icon, String)],
    page: ModuleId,
    indicators: &[Indicator],
) {
    // header: tabs on the left, status on the right
    let hy = full.y + 7.0;
    let ts = 30.0;
    let mut x = full.x + PAD;
    for (id, icon, _title) in tabs {
        let r = Rect::new(x, hy, ts, ts);
        let selected = *id == page;
        let style = ButtonStyle {
            fg: if selected { palette::TEXT } else { palette::TEXT_DIM },
            bg: if selected { Color::white(0.13) } else { Color::white(0.0) },
            hover_bg: if selected { Color::white(0.16) } else { Color::white(0.08) },
            radius: 10.0,
            icon_scale: 0.52,
        };
        if ui.icon_button(r, *icon, style) {
            ui.fx.open_page = Some(id);
        }
        x += ts + 4.0;
    }
    // status (right)
    let mut rx = full.right() - PAD;
    for ind in indicators.iter().rev() {
        if let Some((level, charging)) = ind.battery {
            let label = format!("{:.0}%", level * 100.0);
            let st = TextStyle::new(12.5, palette::TEXT_DIM).medium().right();
            let (tw, _) = ui.p.measure(&label, &st);
            let br = Rect::new(rx - 30.0, hy + ts * 0.5 - 7.0, 30.0, 14.0);
            icons::battery(ui.p, br, level, charging, battery_color(level, charging));
            ui.p.text(&label, Rect::new(br.x - tw - 6.0, hy, tw + 2.0, ts), &st);
            rx = br.x - tw - 16.0;
        } else if let Some(icon) = ind.icon {
            let r = Rect::new(rx - 22.0, hy + 4.0, 22.0, 22.0);
            ui.p.fill_rounded(r, 11.0, ind.color.with_a(0.18));
            ui.p.icon(icon, r.inset(4.5), ind.color);
            rx -= 28.0;
        }
    }

    let content = Rect::new(full.x + PAD, full.y + HEADER_H, full.w - 2.0 * PAD, full.h - HEADER_H - BOTTOM_PAD);
    if page == HOME {
        draw_home(ui, content, modules, started);
    } else if let Some(i) = modules.iter().position(|m| m.id() == page) {
        modules[i].draw_page(ui, content);
    }
}

fn draw_home(ui: &mut Ui, r: Rect, modules: &mut [Box<dyn Module>], started: &[bool]) {
    let large = (0..modules.len()).find(|&i| started[i] && modules[i].card() == Some(CardSize::Large));
    let smalls: Vec<usize> = (0..modules.len())
        .filter(|&i| started[i] && modules[i].card() == Some(CardSize::Small))
        .take(HOME_TILES)
        .collect();
    let (left, right) = if smalls.is_empty() { (r, Rect::default()) } else { r.split_left((r.w * 0.58).round(), 12.0) };
    match large {
        Some(i) => modules[i].draw_card(ui, left),
        None => draw_clock_card(ui, left),
    }
    if !smalls.is_empty() {
        let n = smalls.len() as f32;
        let gap = 8.0;
        let ch = (right.h - gap * (n - 1.0)) / n;
        for (k, &i) in smalls.iter().enumerate() {
            let cr = Rect::new(right.x, right.y + k as f32 * (ch + gap), right.w, ch);
            modules[i].draw_card(ui, cr);
        }
    }
}

fn draw_clock_card(ui: &mut Ui, r: Rect) {
    ui.card(r, false);
    let st = unsafe { GetLocalTime() };
    let mut tbuf = [0u16; 64];
    let mut dbuf = [0u16; 128];
    let tn = unsafe { GetTimeFormatEx(PCWSTR::null(), TIME_NOSECONDS, Some(&st), PCWSTR::null(), Some(&mut tbuf)) };
    let dn = unsafe {
        GetDateFormatEx(PCWSTR::null(), DATE_LONGDATE, Some(&st), PCWSTR::null(), Some(&mut dbuf), PCWSTR::null())
    };
    let time = String::from_utf16_lossy(&tbuf[..(tn.max(1) - 1) as usize]);
    let date = String::from_utf16_lossy(&dbuf[..(dn.max(1) - 1) as usize]);
    let inner = r.inset_xy(18.0, 14.0);
    ui.p.text(
        &time,
        Rect::new(inner.x, inner.y, inner.w, 50.0),
        &TextStyle::new(42.0, palette::TEXT).semibold().display(),
    );
    ui.p.text(&date, Rect::new(inner.x, inner.y + 52.0, inner.w, 20.0), &TextStyle::new(13.0, palette::TEXT_DIM));
    let hint = Rect::new(inner.x, inner.bottom() - 20.0, inner.w, 20.0);
    ui.p.icon(Icon::Music, Rect::new(hint.x, hint.y + 2.0, 16.0, 16.0), palette::TEXT_FAINT);
    ui.p.text(
        "Nothing playing",
        Rect::new(hint.x + 22.0, hint.y, hint.w - 22.0, 20.0),
        &TextStyle::new(12.0, palette::TEXT_FAINT),
    );
}

// ---------------------------------------------------------------------------
// Win32 helpers
// ---------------------------------------------------------------------------

unsafe extern "system" fn win_event_cb(
    _hook: HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    id_object: i32,
    id_child: i32,
    _thread: u32,
    _time: u32,
) {
    if event == EVENT_OBJECT_LOCATIONCHANGE
        && (id_object != OBJID_WINDOW.0 || id_child != CHILDID_SELF as i32 || hwnd != GetForegroundWindow())
    {
        return;
    }
    let h = NOTCH_HWND.load(Ordering::Acquire);
    if h != 0 {
        let _ = PostMessageW(Some(HWND(h as *mut _)), WM_APP_FOREGROUND, WPARAM(event as usize), LPARAM(0));
    }
}

unsafe fn is_fullscreen_on(mon: HMONITOR, own: HWND) -> bool {
    let fg = GetForegroundWindow();
    if fg.is_invalid() || fg == own {
        return false;
    }
    let mut cls = [0u16; 64];
    let n = GetClassNameW(fg, &mut cls);
    let class = String::from_utf16_lossy(&cls[..n.max(0) as usize]);
    // Desktop, taskbar and shell overlays (Start, Alt+Tab, Task View, flyouts)
    // cover the screen but are not fullscreen apps.
    if matches!(
        class.as_str(),
        "Progman"
            | "WorkerW"
            | "Shell_TrayWnd"
            | "Shell_SecondaryTrayWnd"
            | "XamlExplorerHostIslandWindow"
            | "Windows.UI.Core.CoreWindow"
            | "MultitaskingViewFrame"
            | "TaskSwitcherWnd"
            | "ForegroundStaging"
            | "TopLevelWindowForOverflowXamlIsland"
    ) {
        return false;
    }
    if MonitorFromWindow(fg, MONITOR_DEFAULTTONEAREST) != mon {
        return false;
    }
    // The shell's own verdict covers D3D exclusive fullscreen and presentations.
    if let Ok(state) = SHQueryUserNotificationState() {
        if state == QUNS_RUNNING_D3D_FULL_SCREEN || state == QUNS_PRESENTATION_MODE {
            return true;
        }
    }
    // A maximized window is never "fullscreen" (matters with an auto-hide taskbar).
    if IsZoomed(fg).as_bool() {
        return false;
    }
    let mut wr = RECT::default();
    if GetWindowRect(fg, &mut wr).is_err() {
        return false;
    }
    let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
    if !GetMonitorInfoW(mon, &mut mi).as_bool() {
        return false;
    }
    let m = mi.rcMonitor;
    let style = GetWindowLongW(fg, GWL_STYLE) as u32;
    let has_caption = style & WS_CAPTION.0 == WS_CAPTION.0;
    let fs = wr.left <= m.left && wr.top <= m.top && wr.right >= m.right && wr.bottom >= m.bottom && !has_caption;
    if fs {
        crate::log!("fullscreen window: class={class} rect=({},{},{},{})", wr.left, wr.top, wr.right, wr.bottom);
    }
    fs
}

fn monitors() -> Vec<HMONITOR> {
    unsafe extern "system" fn cb(m: HMONITOR, _: HDC, _: *mut RECT, lp: LPARAM) -> windows::core::BOOL {
        let v = &mut *(lp.0 as *mut Vec<HMONITOR>);
        v.push(m);
        true.into()
    }
    let mut v: Vec<HMONITOR> = Vec::new();
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(cb), LPARAM(&mut v as *mut _ as isize));
    }
    v
}

fn key_input(vk: VIRTUAL_KEY, up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: 0,
                dwFlags: if up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn translate_key(vk: u32) -> Option<Key> {
    let ctrl = unsafe { GetKeyState(VK_CONTROL.0 as i32) } < 0;
    let shift = unsafe { GetKeyState(VK_SHIFT.0 as i32) } < 0;
    let vk = VIRTUAL_KEY(vk as u16);
    Some(match vk {
        VK_RETURN => Key::Enter { shift },
        VK_BACK => Key::Backspace,
        VK_DELETE => Key::Delete,
        VK_LEFT => Key::Left,
        VK_RIGHT => Key::Right,
        VK_HOME => Key::Home,
        VK_END => Key::End,
        VK_UP => Key::Up,
        VK_DOWN => Key::Down,
        VK_ESCAPE => Key::Escape,
        VK_TAB => Key::Tab,
        _ if ctrl && vk == VIRTUAL_KEY(b'A' as u16) => Key::SelectAll,
        _ if ctrl && vk == VIRTUAL_KEY(b'C' as u16) => Key::Copy,
        _ if ctrl && vk == VIRTUAL_KEY(b'V' as u16) => Key::Paste,
        _ if ctrl && vk == VIRTUAL_KEY(b'X' as u16) => Key::Cut,
        _ => return None,
    })
}

/// Banner for `dynamic-notch claude <event> <project> <message> <hwnd>` (see claude.rs).
fn claude_peek(args: &[String]) -> Peek {
    const CLAUDE: Color = Color::hex(0xD97757);
    let arg = |i: usize| args.get(i).map(|s| s.trim()).unwrap_or("");
    let (event, project, message) = (arg(0).to_ascii_lowercase(), arg(1), arg(2));
    let hwnd = arg(3).parse::<isize>().unwrap_or(0);
    let join = |a: &str, b: &str| match (a.is_empty(), b.is_empty()) {
        (false, false) => format!("{a} · {b}"),
        _ => format!("{a}{b}"),
    };
    let peek = if matches!(event.as_str(), "notification" | "attention" | "permission") {
        let msg = if message.is_empty() { "Waiting for your input" } else { message };
        Peek::new(Icon::Bell, CLAUDE, "Claude needs you", join(project, msg)).duration_ms(7000)
    } else {
        let sub = if project.is_empty() { "Ready for your next message".to_string() } else { project.to_string() };
        Peek::new(Icon::Sparkle, CLAUDE, "Claude finished", sub).duration_ms(4500)
    };
    peek.key("claude").focus(hwnd)
}

/// Map an IPC page name onto a static module id.
fn leak_page(p: &str) -> ModuleId {
    match p {
        "media" => "media",
        "timer" => "timer",
        "clipboard" => "clipboard",
        "downloads" => "downloads",
        "ai" => "ai",
        "voice" => "voice",
        "shelf" => "shelf",
        "ports" => "ports",
        _ => HOME,
    }
}
