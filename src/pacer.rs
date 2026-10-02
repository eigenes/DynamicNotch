//! V-sync frame pacing.
//!
//! A dedicated thread waits for the compositor clock (Windows 11) or
//! DwmFlush (fallback) and posts `WM_APP_FRAME` to the UI thread — only while
//! an animation is running.  When idle the thread blocks on an event, so an
//! idle notch costs exactly zero CPU.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use windows::core::s;
use windows::Win32::Foundation::{CloseHandle, HANDLE, LPARAM, WPARAM};
use windows::Win32::Graphics::Dwm::DwmFlush;
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::Win32::System::Threading::{CreateEventW, SetEvent, WaitForSingleObject, INFINITE};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

use crate::util::SendHwnd;
use crate::window::WM_APP_FRAME;

type WaitClockFn = unsafe extern "system" fn(u32, *const HANDLE, u32) -> u32;

struct Shared {
    active: AtomicBool,
    pending: AtomicBool,
    quit: AtomicBool,
    wake: isize, // HANDLE
}

pub struct Pacer {
    shared: Arc<Shared>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Pacer {
    pub fn new(hwnd: SendHwnd) -> Self {
        let ev = unsafe { CreateEventW(None, false, false, None).expect("CreateEvent") };
        let shared = Arc::new(Shared {
            active: AtomicBool::new(false),
            pending: AtomicBool::new(false),
            quit: AtomicBool::new(false),
            wake: ev.0 as isize,
        });
        let s2 = shared.clone();
        let thread = std::thread::Builder::new().name("vsync".into()).spawn(move || run(s2, hwnd)).ok();
        Self { shared, thread }
    }

    /// Keep frames coming until `stop` is called.
    pub fn start(&self) {
        if !self.shared.active.swap(true, Ordering::AcqRel) {
            unsafe {
                let _ = SetEvent(HANDLE(self.shared.wake as *mut _));
            }
        }
    }

    pub fn stop(&self) {
        self.shared.active.store(false, Ordering::Release);
    }

    /// The UI thread consumed a frame message.
    pub fn frame_consumed(&self) {
        self.shared.pending.store(false, Ordering::Release);
    }
}

impl Drop for Pacer {
    fn drop(&mut self) {
        self.shared.quit.store(true, Ordering::Release);
        unsafe {
            let _ = SetEvent(HANDLE(self.shared.wake as *mut _));
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        unsafe {
            let _ = CloseHandle(HANDLE(self.shared.wake as *mut _));
        }
    }
}

fn load_wait_clock() -> Option<WaitClockFn> {
    unsafe {
        let lib = LoadLibraryW(windows::core::w!("dcomp.dll")).ok()?;
        let f = GetProcAddress(lib, s!("DCompositionWaitForCompositorClock"))?;
        Some(std::mem::transmute::<_, WaitClockFn>(f))
    }
}

fn run(s: Arc<Shared>, hwnd: SendHwnd) {
    let wait_clock = load_wait_clock();
    let wake = HANDLE(s.wake as *mut _);
    let mut last = Instant::now();
    loop {
        if s.quit.load(Ordering::Acquire) {
            break;
        }
        if !s.active.load(Ordering::Acquire) {
            unsafe { WaitForSingleObject(wake, INFINITE) };
            last = Instant::now();
            continue;
        }
        // Wait for the next composition frame.
        match wait_clock {
            Some(f) => unsafe {
                f(0, std::ptr::null(), 50);
            },
            None => unsafe {
                let _ = DwmFlush();
            },
        }
        // Guard against waits that return immediately (no composition
        // happening, remote sessions...): never exceed ~250 fps.
        let el = last.elapsed();
        if el < Duration::from_millis(4) {
            std::thread::sleep(Duration::from_millis(4) - el);
        }
        last = Instant::now();
        if !s.pending.swap(true, Ordering::AcqRel) {
            unsafe {
                let _ = PostMessageW(Some(hwnd.hwnd()), WM_APP_FRAME, WPARAM(0), LPARAM(0));
            }
        }
    }
}
