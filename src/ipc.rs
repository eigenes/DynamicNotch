//! Single-instance handling and the command-line API.
//!
//! Running `dynamic-notch.exe <command> ...` while the notch is already
//! running forwards the command via WM_COPYDATA. This makes the notch
//! scriptable from anything (build scripts, AutoHotkey, Task Scheduler...):
//!
//!   dynamic-notch notify "Build finished" "All 112 tests passed"
//!   dynamic-notch progress "Rendering" 42        (0-100, "done" or "cancel")
//!   dynamic-notch timer 25                        (start a 25 min timer)
//!   dynamic-notch ask "convert 72F to C"           (ask the AI)
//!   dynamic-notch toggle | expand | collapse | ai | reload | quit

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS, HANDLE, LPARAM, WPARAM};
use windows::Win32::System::DataExchange::COPYDATASTRUCT;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, SendMessageTimeoutW, SMTO_ABORTIFHUNG, WM_COPYDATA};

use crate::window::CLASS_NAME;

pub const COPYDATA_MAGIC: usize = 0x4E4F_5443; // "NOTC"

pub const HELP: &str = "Dynamic Notch — a MacBook-style notch for Windows

USAGE:
  dynamic-notch                      start (or bring up) the notch
  dynamic-notch notify <title> [body]
  dynamic-notch progress <name> <0-100|done|cancel>
  dynamic-notch timer <minutes>
  dynamic-notch ask <prompt>         ask the AI (OpenRouter)
  dynamic-notch claude [--always]    Claude Code hook (reads the hook JSON on stdin)
  dynamic-notch toggle | expand | collapse | ai | reload | quit
";

/// Returns the mutex handle if we are the first instance.
pub fn acquire_single_instance() -> Option<HANDLE> {
    unsafe {
        let h = CreateMutexW(None, true, w!("Local\\DynamicNotch.SingleInstance")).ok()?;
        if GetLastError() == ERROR_ALREADY_EXISTS {
            None
        } else {
            Some(h)
        }
    }
}

/// Forward `args` to the running instance. Returns false if none is running.
pub fn send_to_running(args: &[String]) -> bool {
    unsafe {
        let Ok(hwnd) = FindWindowW(CLASS_NAME, PCWSTR::null()) else { return false };
        let payload = serde_json::to_vec(args).unwrap_or_default();
        let cds =
            COPYDATASTRUCT { dwData: COPYDATA_MAGIC, cbData: payload.len() as u32, lpData: payload.as_ptr() as *mut _ };
        let mut result = 0usize;
        let r = SendMessageTimeoutW(
            hwnd,
            WM_COPYDATA,
            WPARAM(0),
            LPARAM(&cds as *const _ as isize),
            SMTO_ABORTIFHUNG,
            2000,
            Some(&mut result),
        );
        if cfg!(debug_assertions) {
            eprintln!("ipc: sent to {hwnd:?} -> {} (result {result})", r.0);
        }
        r.0 != 0
    }
}

/// Decode a WM_COPYDATA payload produced by `send_to_running`.
pub fn decode(lp: LPARAM) -> Option<Vec<String>> {
    unsafe {
        let cds = &*(lp.0 as *const COPYDATASTRUCT);
        if cds.dwData != COPYDATA_MAGIC || cds.lpData.is_null() {
            return None;
        }
        let bytes = std::slice::from_raw_parts(cds.lpData as *const u8, cds.cbData as usize);
        serde_json::from_slice(bytes).ok()
    }
}
