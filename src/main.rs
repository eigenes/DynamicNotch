//! Dynamic Notch — a MacBook-style notch / desktop island for Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
#![deny(unreachable_patterns)]

mod anim;
mod app;
mod bus;
mod config;
mod gfx;
mod host;
mod hotkeys;
mod ipc;
mod modules;
mod pacer;
mod sys;
mod tray;
mod ui;
mod util;
mod window;

use windows::Win32::System::Console::{AttachConsole, ATTACH_PARENT_PROCESS};
use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_SINGLETHREADED};
use windows::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};

use crate::config::LoadResult;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| matches!(a.as_str(), "-h" | "--help" | "help" | "/?")) {
        unsafe {
            let _ = AttachConsole(ATTACH_PARENT_PROCESS);
        }
        println!("{}", ipc::HELP);
        return;
    }

    let Some(_instance) = ipc::acquire_single_instance() else {
        // Already running: hand our command line to the existing instance.
        let cmd = if args.is_empty() { vec!["expand".to_string()] } else { args };
        ipc::send_to_running(&cmd);
        return;
    };

    util::log_init();
    std::panic::set_hook(Box::new(|info| {
        log!("PANIC: {info}");
    }));

    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let _ = RoInitialize(RO_INIT_SINGLETHREADED);
    }

    let cfg = match config::load() {
        LoadResult::Ok(c) => c,
        LoadResult::Invalid(e) => {
            log!("config error, using defaults: {e}");
            config::Config::default()
        }
    };

    if let Err(e) = run(cfg, args) {
        log!("fatal: {e}");
        let msg = util::wide(&format!("Dynamic Notch failed to start:\n\n{e}"));
        unsafe {
            MessageBoxW(
                None,
                windows::core::PCWSTR(msg.as_ptr()),
                windows::core::w!("Dynamic Notch"),
                MB_OK | MB_ICONERROR,
            );
        }
    }
}

fn run(cfg: config::Config, args: Vec<String>) -> windows::core::Result<()> {
    let hwnd = window::create()?;
    let app = app::App::new(hwnd, cfg)?;
    window::APP.with(|a| *a.borrow_mut() = Some(app));
    window::with_app(|a| a.start());
    if !args.is_empty() {
        window::with_app(|a| a.on_ipc(args));
    }
    window::run_message_loop();
    window::APP.with(|a| drop(a.borrow_mut().take()));
    Ok(())
}
