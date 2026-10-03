//! Native Windows integration helpers shared by the shell and modules.

pub mod audio;
pub mod clip;
pub mod dotenv;
pub mod dragdrop;
pub mod http;
pub mod stt;

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

/// Bring a top-level window to the front (restoring it if minimized). Works
/// right after a click on the notch, which grants foreground rights.
pub fn activate_window(hwnd: isize) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{IsIconic, IsWindow, SetForegroundWindow, ShowWindow, SW_RESTORE};
    let h = HWND(hwnd as *mut _);
    unsafe {
        if !IsWindow(Some(h)).as_bool() {
            return;
        }
        if IsIconic(h).as_bool() {
            let _ = ShowWindow(h, SW_RESTORE);
        }
        let _ = SetForegroundWindow(h);
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

/// Play a system sound alias with its samples scaled by `gain`. The gain is
/// capped so the loudest sample doesn't clip. Falls back to plain
/// `play_sound` when the alias isn't a 16-bit PCM wav.
pub fn play_sound_gain(alias: &str, gain: f32) {
    use windows::Win32::Media::Audio::{PlaySoundW, SND_ASYNC, SND_MEMORY, SND_NODEFAULT};
    // PlaySound reads an async SND_MEMORY buffer while it plays, so it must outlive the call.
    static BUF: std::sync::Mutex<Vec<u8>> = std::sync::Mutex::new(Vec::new());

    let wav = (gain - 1.0).abs() > 0.01;
    let Some(mut data) = wav.then(|| alias_path(alias)).flatten().and_then(|p| std::fs::read(p).ok()) else {
        return play_sound(alias);
    };
    if !amplify_wav(&mut data, gain) {
        return play_sound(alias);
    }
    let Ok(mut buf) = BUF.lock() else { return play_sound(alias) };
    unsafe {
        // stop anything still reading the old buffer before replacing it
        let _ = PlaySoundW(PCWSTR::null(), None, Default::default());
        *buf = data;
        let _ = PlaySoundW(PCWSTR(buf.as_ptr() as *const u16), None, SND_MEMORY | SND_ASYNC | SND_NODEFAULT);
    }
}

/// The .wav file the current sound scheme assigns to `alias`.
fn alias_path(alias: &str) -> Option<String> {
    let key = wide(&format!(r"AppEvents\Schemes\Apps\.Default\{alias}\.Current"));
    let mut buf = [0u16; 520];
    let mut size = (buf.len() * 2) as u32;
    let ok = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            PCWSTR(key.as_ptr()),
            PCWSTR::null(),
            RRF_RT_REG_SZ | windows::Win32::System::Registry::RRF_RT_REG_EXPAND_SZ,
            None,
            Some(buf.as_mut_ptr().cast()),
            Some(&mut size),
        ) == WIN32_ERROR(0)
    };
    let len = buf.iter().position(|&c| c == 0).unwrap_or(0);
    let mut path = String::from_utf16_lossy(&buf[..len]);
    if !ok || path.is_empty() {
        return None;
    }
    for var in ["SystemRoot", "windir"] {
        if let Ok(v) = std::env::var(var) {
            path = path.replace(&format!("%{var}%"), &v);
        }
    }
    Some(path)
}

/// Scale the samples of a 16-bit PCM RIFF/WAVE in place. Returns false if the
/// file isn't in that format.
fn amplify_wav(wav: &mut [u8], gain: f32) -> bool {
    if wav.len() < 12 || &wav[0..4] != b"RIFF" || &wav[8..12] != b"WAVE" {
        return false;
    }
    let u16_at = |b: &[u8], i: usize| u16::from_le_bytes([b[i], b[i + 1]]);
    let (mut pos, mut pcm16) = (12usize, false);
    while pos + 8 <= wav.len() {
        let id: [u8; 4] = wav[pos..pos + 4].try_into().unwrap();
        let len = u32::from_le_bytes(wav[pos + 4..pos + 8].try_into().unwrap()) as usize;
        let body = pos + 8;
        let end = (body + len).min(wav.len());
        match &id {
            b"fmt " if len >= 16 && end >= body + 16 => {
                pcm16 = u16_at(wav, body) == 1 && u16_at(wav, body + 14) == 16;
            }
            b"data" if pcm16 => {
                let samples = &mut wav[body..end - (end - body) % 2];
                let peak = samples.chunks_exact(2).map(|s| i16::from_le_bytes([s[0], s[1]]).unsigned_abs()).max();
                let limit = 32_000.0 / peak.unwrap_or(1).max(1) as f32;
                let g = gain.max(0.0).min(limit);
                for s in samples.chunks_exact_mut(2) {
                    let v = (i16::from_le_bytes([s[0], s[1]]) as f32 * g).round().clamp(-32768.0, 32767.0) as i16;
                    s.copy_from_slice(&v.to_le_bytes());
                }
                return true;
            }
            _ => {}
        }
        pos = body + len + (len & 1);
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wav(samples: &[i16]) -> Vec<u8> {
        let data: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        let mut w = b"RIFF".to_vec();
        w.extend((36 + data.len() as u32).to_le_bytes());
        w.extend(b"WAVEfmt ");
        w.extend(16u32.to_le_bytes());
        w.extend([1, 0, 1, 0]); // PCM, mono
        w.extend(44_100u32.to_le_bytes());
        w.extend(88_200u32.to_le_bytes());
        w.extend([2, 0, 16, 0]); // block align, 16 bits
        w.extend(b"data");
        w.extend((data.len() as u32).to_le_bytes());
        w.extend(data);
        w
    }

    #[test]
    fn amplifies_without_clipping() {
        let mut w = wav(&[1000, -2000]);
        assert!(amplify_wav(&mut w, 2.0));
        assert_eq!(&w[44..], &wav(&[2000, -4000])[44..]);

        let mut w = wav(&[16_000]);
        assert!(amplify_wav(&mut w, 4.0));
        assert_eq!(i16::from_le_bytes([w[44], w[45]]), 32_000);

        assert!(!amplify_wav(&mut b"not a wav file".to_vec(), 2.0));
    }
}
