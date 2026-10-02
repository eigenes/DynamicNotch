//! Minimal clipboard read/write helpers shared by modules.

use std::time::Duration;

use windows::Win32::Foundation::{HANDLE, HGLOBAL, HWND};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE};
use windows::Win32::System::Ole::CF_UNICODETEXT;

fn open(hwnd: HWND) -> bool {
    for _ in 0..5 {
        if unsafe { OpenClipboard(Some(hwnd)) }.is_ok() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(15));
    }
    false
}

/// Run `f` over the bytes of a clipboard/global memory handle.
pub unsafe fn with_global<R>(h: HANDLE, f: impl FnOnce(&[u8]) -> R) -> Option<R> {
    let g = HGLOBAL(h.0);
    let p = GlobalLock(g);
    if p.is_null() {
        return None;
    }
    let n = GlobalSize(g);
    let r = f(std::slice::from_raw_parts(p as *const u8, n));
    let _ = GlobalUnlock(g);
    Some(r)
}

pub fn get_text(hwnd: HWND) -> Option<String> {
    if !open(hwnd) {
        return None;
    }
    let r = unsafe {
        GetClipboardData(CF_UNICODETEXT.0 as u32).ok().and_then(|h| {
            with_global(h, |b| {
                let w: Vec<u16> =
                    b.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|&c| c != 0).collect();
                String::from_utf16_lossy(&w)
            })
        })
    };
    unsafe {
        let _ = CloseClipboard();
    }
    r
}

pub fn set_text(hwnd: HWND, text: &str) -> bool {
    let w: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
    let bytes: Vec<u8> = w.iter().flat_map(|c| c.to_le_bytes()).collect();
    set_data(hwnd, CF_UNICODETEXT.0 as u32, &bytes)
}

pub fn set_data(hwnd: HWND, format: u32, bytes: &[u8]) -> bool {
    if !open(hwnd) {
        return false;
    }
    let ok = unsafe {
        (|| {
            EmptyClipboard().ok()?;
            let g = GlobalAlloc(GMEM_MOVEABLE, bytes.len()).ok()?;
            let p = GlobalLock(g);
            if p.is_null() {
                return None;
            }
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), p as *mut u8, bytes.len());
            let _ = GlobalUnlock(g);
            SetClipboardData(format, Some(HANDLE(g.0))).ok()?;
            Some(())
        })()
        .is_some()
    };
    unsafe {
        let _ = CloseClipboard();
    }
    ok
}
