//! Win32 window plumbing. The window procedure forwards everything to the
//! `App` living in a thread-local; re-entrant messages (sent synchronously
//! while the app is busy) fall back to `DefWindowProc`.

use std::cell::RefCell;

use windows::core::{w, Result, BOOL, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DwmSetWindowAttribute, DWMWA_BORDER_COLOR, DWMWA_EXCLUDED_FROM_PEEK, DWMWA_USE_HOSTBACKDROPBRUSH,
    DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::app::App;

pub const WM_APP_FRAME: u32 = WM_APP + 1;
pub const WM_APP_BUS: u32 = WM_APP + 2;
pub const WM_APP_TRAY: u32 = WM_APP + 3;
pub const WM_APP_FOREGROUND: u32 = WM_APP + 4;

pub const CLASS_NAME: PCWSTR = w!("DynamicNotch.Window");

thread_local! {
    pub static APP: RefCell<Option<Box<App>>> = const { RefCell::new(None) };
}

pub fn create() -> Result<HWND> {
    unsafe {
        let hinst = GetModuleHandleW(None)?;
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: hinst.into(),
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            lpszClassName: CLASS_NAME,
            ..Default::default()
        };
        RegisterClassExW(&wc);
        let hwnd = CreateWindowExW(
            WS_EX_NOREDIRECTIONBITMAP | WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE,
            CLASS_NAME,
            w!("Dynamic Notch"),
            WS_POPUP,
            0,
            0,
            1,
            1,
            None,
            None,
            Some(hinst.into()),
            None,
        )?;
        let no_round = DWMWCP_DONOTROUND;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &no_round as *const _ as _,
            std::mem::size_of_val(&no_round) as u32,
        );
        let on = BOOL(1);
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_USE_HOSTBACKDROPBRUSH, &on as *const _ as _, 4);
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_EXCLUDED_FROM_PEEK, &on as *const _ as _, 4);
        let none: u32 = 0xFFFF_FFFE; // DWMWA_COLOR_NONE
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_BORDER_COLOR, &none as *const _ as _, 4);
        Ok(hwnd)
    }
}

/// Run `f` with the app if it is not already borrowed.
pub fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|a| match a.try_borrow_mut() {
        Ok(mut g) => g.as_mut().map(|app| f(app)),
        Err(_) => None,
    })
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    // A panic must never unwind into Windows (that aborts the process); log it
    // and keep the notch alive instead.
    match std::panic::catch_unwind(|| wndproc_inner(hwnd, msg, wp, lp)) {
        Ok(r) => r,
        Err(_) => {
            crate::log!("recovered from panic while handling message {msg:#x}");
            unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
        }
    }
}

fn wndproc_inner(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    // Context menus run a modal loop, so they must be shown *outside* the
    // app borrow; otherwise frames/timers arriving meanwhile would be lost.
    let menu_request = match msg {
        WM_APP_TRAY => {
            let ev = (lp.0 as u32) & 0xFFFF;
            ev == WM_CONTEXTMENU || ev == WM_RBUTTONUP
        }
        WM_RBUTTONUP => true,
        _ => false,
    };
    if menu_request {
        crate::tray::show_menu(hwnd);
        return LRESULT(0);
    }

    match with_app(|app| app.handle(msg, wp, lp)) {
        Some(Some(r)) => return r,
        Some(None) => {}
        None => {
            if cfg!(debug_assertions) && msg >= WM_APP || msg == WM_COPYDATA {
                crate::log!("re-entrant message {msg:#x} dropped");
            }
        }
    }
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}

pub fn run_message_loop() {
    unsafe {
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}
