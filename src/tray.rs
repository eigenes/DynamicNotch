//! System tray icon + context menu (also shown on right-click of the notch).

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CreateBitmap, CreateDIBSection, DeleteObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS,
};
use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NIM_SETVERSION,
    NOTIFYICONDATAW, NOTIFYICON_VERSION_4,
};
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::util::wide;
use crate::window::{with_app, WM_APP_TRAY};

pub const CMD_TOGGLE: u32 = 1;
pub const CMD_AI: u32 = 2;
pub const CMD_HIDE: u32 = 3;
pub const CMD_TIMER: u32 = 4;
pub const CMD_MODULE_BASE: u32 = 100;
pub const CMD_MON_PRIMARY: u32 = 200;
pub const CMD_MON_ACTIVE: u32 = 201;
pub const CMD_MON_BASE: u32 = 210;
pub const CMD_AUTOSTART: u32 = 300;
pub const CMD_BLUR: u32 = 301;
pub const CMD_HOVER: u32 = 302;
pub const CMD_FULLSCREEN: u32 = 303;
pub const CMD_EDIT_CONFIG: u32 = 400;
pub const CMD_OPEN_FOLDER: u32 = 401;
pub const CMD_RELOAD: u32 = 402;
pub const CMD_QUIT: u32 = 999;

pub struct MenuState {
    pub expanded: bool,
    pub user_hidden: bool,
    pub modules: Vec<(String, bool)>,
    pub monitor_count: usize,
    pub monitor: String,
    pub autostart: bool,
    pub blur: bool,
    pub hover: bool,
    pub hide_fullscreen: bool,
}

pub struct Tray {
    hwnd: HWND,
    icon: HICON,
    added: bool,
}

impl Tray {
    pub fn new(hwnd: HWND) -> Self {
        let icon = make_icon().unwrap_or_default();
        let mut t = Self { hwnd, icon, added: false };
        t.add();
        t
    }

    fn data(&self) -> NOTIFYICONDATAW {
        let mut nid = NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: self.hwnd,
            uID: 1,
            uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP | NIF_SHOWTIP,
            uCallbackMessage: WM_APP_TRAY,
            hIcon: self.icon,
            ..Default::default()
        };
        let tip = wide("Dynamic Notch");
        nid.szTip[..tip.len()].copy_from_slice(&tip);
        nid
    }

    pub fn add(&mut self) {
        unsafe {
            let mut nid = self.data();
            self.added = Shell_NotifyIconW(NIM_ADD, &nid).as_bool();
            nid.Anonymous.uVersion = NOTIFYICON_VERSION_4;
            let _ = Shell_NotifyIconW(NIM_SETVERSION, &nid);
        }
    }

    /// Explorer restarted ("TaskbarCreated"): icon must be re-added.
    pub fn readd(&mut self) {
        self.added = false;
        self.add();
    }

    /// Refresh the icon (e.g. after a light/dark theme switch).
    pub fn refresh_icon(&mut self) {
        if let Some(i) = make_icon() {
            unsafe {
                let _ = DestroyIcon(self.icon);
            }
            self.icon = i;
            let nid = self.data();
            unsafe {
                let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
            }
        }
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        unsafe {
            let nid = self.data();
            let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
            let _ = DestroyIcon(self.icon);
        }
    }
}

fn append(menu: HMENU, id: u32, text: &str, checked: bool) {
    let t = wide(text);
    let flags = MF_STRING | if checked { MF_CHECKED } else { MF_UNCHECKED };
    unsafe {
        let _ = AppendMenuW(menu, flags, id as usize, PCWSTR(t.as_ptr()));
    }
}

fn separator(menu: HMENU) {
    unsafe {
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
    }
}

fn build(s: &MenuState) -> HMENU {
    unsafe {
        let menu = CreatePopupMenu().unwrap_or_default();
        append(menu, CMD_TOGGLE, if s.expanded { "Collapse notch" } else { "Open notch" }, false);
        append(menu, CMD_AI, "Ask AI…", false);
        append(menu, CMD_TIMER, "Timer", false);
        append(menu, CMD_HIDE, "Hide notch", s.user_hidden);
        separator(menu);

        let modules = CreatePopupMenu().unwrap_or_default();
        for (i, (name, on)) in s.modules.iter().enumerate() {
            append(modules, CMD_MODULE_BASE + i as u32, name, *on);
        }
        let mt = wide("Modules");
        let _ = AppendMenuW(menu, MF_POPUP, modules.0 as usize, PCWSTR(mt.as_ptr()));

        let mons = CreatePopupMenu().unwrap_or_default();
        append(mons, CMD_MON_PRIMARY, "Primary monitor", s.monitor == "primary");
        append(mons, CMD_MON_ACTIVE, "Follow active window", s.monitor == "active");
        separator(mons);
        for i in 0..s.monitor_count {
            append(mons, CMD_MON_BASE + i as u32, &format!("Monitor {}", i + 1), s.monitor == (i + 1).to_string());
        }
        let dt = wide("Display");
        let _ = AppendMenuW(menu, MF_POPUP, mons.0 as usize, PCWSTR(dt.as_ptr()));

        append(menu, CMD_HOVER, "Expand on hover", s.hover);
        append(menu, CMD_BLUR, "Glass blur", s.blur);
        append(menu, CMD_FULLSCREEN, "Hide in fullscreen apps", s.hide_fullscreen);
        append(menu, CMD_AUTOSTART, "Start with Windows", s.autostart);
        separator(menu);
        append(menu, CMD_EDIT_CONFIG, "Edit settings…", false);
        append(menu, CMD_OPEN_FOLDER, "Open settings folder", false);
        append(menu, CMD_RELOAD, "Reload settings", false);
        separator(menu);
        append(menu, CMD_QUIT, "Quit", false);
        menu
    }
}

/// Show the context menu at the cursor. Must be called outside the app borrow.
pub fn show_menu(hwnd: HWND) {
    let Some(state) = with_app(|a| a.menu_state()) else { return };
    let menu = build(&state);
    unsafe {
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        // Required so the menu closes when clicking elsewhere.
        let _ = SetForegroundWindow(hwnd);
        let cmd = TrackPopupMenuEx(
            menu,
            (TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN).0,
            pt.x,
            pt.y,
            hwnd,
            None,
        );
        let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(menu);
        if cmd.0 != 0 {
            with_app(|a| a.on_menu(cmd.0 as u32));
        }
    }
}

fn light_taskbar() -> bool {
    let mut v: u32 = 0;
    let mut sz = 4u32;
    unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("SystemUsesLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut v as *mut u32 as *mut _),
            Some(&mut sz),
        )
        .is_ok()
            && v == 1
    }
}

/// Draw the tray icon: a little "island" capsule with a lens dot.
fn make_icon() -> Option<HICON> {
    let size = unsafe { GetSystemMetrics(SM_CXSMICON) }.clamp(16, 64) as i32;
    let light = light_taskbar();
    let (body, dot) =
        if light { ([0x16u8, 0x16, 0x16], [0x7A, 0x7A, 0x7A]) } else { ([0xF4u8, 0xF4, 0xF4], [0x30, 0x30, 0x30]) };
    let s = size as f32;
    let (cw, ch) = (s * 0.92, s * 0.44);
    let (cx, cy) = (s * 0.5, s * 0.5);
    let r = ch * 0.5;
    let mut px = vec![0u8; (size * size * 4) as usize];
    for y in 0..size {
        for x in 0..size {
            let fx = x as f32 + 0.5;
            let fy = y as f32 + 0.5;
            // signed distance to the capsule
            let qx = ((fx - cx).abs() - (cw * 0.5 - r)).max(0.0);
            let qy = fy - cy;
            let d = (qx * qx + qy * qy).sqrt() - r;
            let a = (0.5 - d).clamp(0.0, 1.0);
            // lens dot on the right
            let dx = fx - (cx + cw * 0.5 - r);
            let dd = (dx * dx + qy * qy).sqrt() - r * 0.42;
            let da = (0.5 - dd).clamp(0.0, 1.0);
            let mix = |b: u8, d: u8| (b as f32 * (1.0 - da) + d as f32 * da) as u8;
            let i = ((y * size + x) * 4) as usize;
            px[i] = mix(body[2], dot[2]);
            px[i + 1] = mix(body[1], dot[1]);
            px[i + 2] = mix(body[0], dot[0]);
            px[i + 3] = (a * 255.0) as u8;
        }
    }
    unsafe {
        let bi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: size,
                biHeight: -size,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let color = CreateDIBSection(None, &bi, DIB_RGB_COLORS, &mut bits, None, 0).ok()?;
        std::ptr::copy_nonoverlapping(px.as_ptr(), bits as *mut u8, px.len());
        let mask_bits = vec![0u8; ((size + 15) / 16 * 2 * size) as usize];
        let mask = CreateBitmap(size, size, 1, 1, Some(mask_bits.as_ptr() as *const _));
        let info = ICONINFO { fIcon: true.into(), xHotspot: 0, yHotspot: 0, hbmMask: mask, hbmColor: color };
        let icon = CreateIconIndirect(&info).ok();
        let _ = DeleteObject(color.into());
        let _ = DeleteObject(mask.into());
        icon
    }
}
