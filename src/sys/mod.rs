//! Native Windows integration helpers shared by the shell and modules.

pub mod clip;
pub mod dotenv;
pub mod http;

use std::time::{Duration, SystemTime};

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::WIN32_ERROR;
use windows::Win32::Storage::FileSystem::{
    FindFirstChangeNotificationW, FindNextChangeNotification, FILE_NOTIFY_CHANGE_FILE_NAME,
    FILE_NOTIFY_CHANGE_LAST_WRITE,
};
use windows::Win32::System::Registry::{
    RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ,
};
use windows::Win32::System::Threading::{WaitForSingleObject, INFINITE};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

use crate::bus::{Bus, Msg};
use crate::config::config_path;
use crate::util::{app_dir, wide};

/// Watch the settings folder and notify the UI thread when config.toml changes.
pub fn watch_config(bus: Bus) {
    let _ = std::thread::Builder::new().name("config-watch".into()).spawn(move || unsafe {
        let dir = wide(&app_dir().to_string_lossy());
        let Ok(h) = FindFirstChangeNotificationW(
            PCWSTR(dir.as_ptr()),
            false,
            FILE_NOTIFY_CHANGE_LAST_WRITE | FILE_NOTIFY_CHANGE_FILE_NAME,
        ) else {
            return;
        };
        let mtime = || std::fs::metadata(config_path()).and_then(|m| m.modified()).ok();
        let mut last: Option<SystemTime> = mtime();
        loop {
            WaitForSingleObject(h, INFINITE);
            // editors often write in several steps; let them finish
            std::thread::sleep(Duration::from_millis(150));
            let now = mtime();
            if now != last {
                last = now;
                bus.send(Msg::ConfigFileChanged);
            }
            if FindNextChangeNotification(h).is_err() {
                break;
            }
        }
    });
}

const RUN_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const RUN_VALUE: PCWSTR = w!("DynamicNotch");

pub fn set_autostart(on: bool) -> bool {
    unsafe {
        if on {
            let Ok(exe) = std::env::current_exe() else { return false };
            let cmd = wide(&format!("\"{}\"", exe.display()));
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                RUN_KEY,
                RUN_VALUE,
                REG_SZ.0,
                Some(cmd.as_ptr() as *const _),
                (cmd.len() * 2) as u32,
            ) == WIN32_ERROR(0)
        } else {
            let _ = RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN_KEY, RUN_VALUE);
            true
        }
    }
}

pub fn is_autostart() -> bool {
    unsafe {
        let mut size = 0u32;
        RegGetValueW(HKEY_CURRENT_USER, RUN_KEY, RUN_VALUE, RRF_RT_REG_SZ, None, None, Some(&mut size))
            == WIN32_ERROR(0)
    }
}

/// Open a file/folder/URL with its default handler.
pub fn shell_open(target: &str) {
    let t = wide(target);
    unsafe {
        ShellExecuteW(None, w!("open"), PCWSTR(t.as_ptr()), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL);
    }
}

/// Reveal a file in Explorer.
pub fn shell_reveal(path: &str) {
    let args = wide(&format!("/select,\"{path}\""));
    unsafe {
        ShellExecuteW(None, w!("open"), w!("explorer.exe"), PCWSTR(args.as_ptr()), PCWSTR::null(), SW_SHOWNORMAL);
    }
}

/// Play a short system sound asynchronously.
pub fn play_sound(alias: &str) {
    let a = wide(alias);
    unsafe {
        let _ = windows::Win32::Media::Audio::PlaySoundW(
            PCWSTR(a.as_ptr()),
            None,
            windows::Win32::Media::Audio::SND_ALIAS | windows::Win32::Media::Audio::SND_ASYNC,
        );
    }
}
