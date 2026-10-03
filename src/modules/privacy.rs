//! Microphone / camera in-use indicators.
//!
//! Windows records capability usage per app under
//! HKCU\...\CapabilityAccessManager\ConsentStore\{microphone,webcam}; an app
//! is currently using the device when `LastUsedTimeStop` is 0. We wait on
//! registry change notifications (no polling) and rescan on change.

use std::any::Any;
use std::time::Duration;

use windows::core::{w, PCWSTR, PWSTR};
use windows::Win32::Foundation::{HANDLE, WIN32_ERROR};
use windows::Win32::System::Registry::*;
use windows::Win32::System::Threading::{CreateEventW, WaitForMultipleObjects, INFINITE};

use super::{Cx, Indicator, Module, ModuleId, Peek};
use crate::bus::Bus;
use crate::gfx::Icon;
use crate::util::{from_wide, palette};

const ID: ModuleId = "privacy";
const BASE: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\CapabilityAccessManager\\ConsentStore\\";

#[derive(Clone, Debug, Default, PartialEq)]
struct Usage {
    mic: Vec<String>,
    cam: Vec<String>,
}

pub struct Privacy {
    usage: Usage,
    started: bool,
}

impl Privacy {
    pub fn new() -> Self {
        Self { usage: Usage::default(), started: false }
    }
}

impl Module for Privacy {
    fn id(&self) -> ModuleId {
        ID
    }
    fn title(&self) -> &str {
        "Mic & Camera"
    }
    fn icon(&self) -> Icon {
        Icon::Mic
    }

    fn start(&mut self, cx: &mut Cx) {
        if self.started {
            return;
        }
        self.started = true;
        let bus = cx.bus.clone();
        let _ = std::thread::Builder::new().name("privacy".into()).spawn(move || worker(bus));
    }

    fn on_message(&mut self, msg: Box<dyn Any + Send>, cx: &mut Cx) {
        let Ok(u) = msg.downcast::<Usage>() else { return };
        let u = *u;
        let new_cam: Vec<&String> = u.cam.iter().filter(|a| !self.usage.cam.contains(a)).collect();
        let new_mic: Vec<&String> = u.mic.iter().filter(|a| !self.usage.mic.contains(a)).collect();
        if !new_cam.is_empty() {
            cx.fx.peek(
                Peek::new(Icon::Camera, palette::GREEN, "Camera in use", join(&new_cam))
                    .key("privacy-cam")
                    .duration_ms(2600),
            );
        } else if !new_mic.is_empty() {
            cx.fx.peek(
                Peek::new(Icon::Mic, palette::ORANGE, "Microphone in use", join(&new_mic))
                    .key("privacy-mic")
                    .duration_ms(2600),
            );
        }
        self.usage = u;
        cx.fx.redraw = true;
    }

    fn indicators(&self, out: &mut Vec<Indicator>) {
        if !self.usage.cam.is_empty() {
            out.push(Indicator {
                color: palette::GREEN,
                icon: Some(Icon::Camera),
                label: Some(self.usage.cam.join(", ")),
                battery: None,
                dot: true,
            });
        }
        if !self.usage.mic.is_empty() {
            out.push(Indicator {
                color: palette::ORANGE,
                icon: Some(Icon::Mic),
                label: Some(self.usage.mic.join(", ")),
                battery: None,
                dot: true,
            });
        }
    }
}

fn join(v: &[&String]) -> String {
    v.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")
}

fn open(path: &str) -> Option<HKEY> {
    let w: Vec<u16> = path.encode_utf16().chain(Some(0)).collect();
    let mut h = HKEY::default();
    let r = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, PCWSTR(w.as_ptr()), Some(0), KEY_READ | KEY_NOTIFY, &mut h) };
    (r == WIN32_ERROR(0)).then_some(h)
}

fn worker(bus: Bus) {
    let keys: Vec<(HKEY, HANDLE)> = ["microphone", "webcam"]
        .iter()
        .filter_map(|k| {
            let h = open(&format!("{BASE}{k}"))?;
            let ev = unsafe { CreateEventW(None, false, false, None).ok()? };
            Some((h, ev))
        })
        .collect();
    if keys.is_empty() {
        crate::log!("privacy: consent store not available");
        return;
    }
    let mut last = Usage::default();
    let mut first = true;
    loop {
        for (h, ev) in &keys {
            unsafe {
                let _ = RegNotifyChangeKeyValue(
                    *h,
                    true,
                    REG_NOTIFY_CHANGE_NAME | REG_NOTIFY_CHANGE_LAST_SET,
                    Some(*ev),
                    true,
                );
            }
        }
        let usage = Usage { mic: scan("microphone"), cam: scan("webcam") };
        if usage != last || first {
            first = false;
            last = usage.clone();
            bus.to_module(ID, usage);
        }
        let handles: Vec<HANDLE> = keys.iter().map(|k| k.1).collect();
        unsafe { WaitForMultipleObjects(&handles, false, INFINITE) };
        // writes come in bursts (start + stop timestamps); let them settle
        std::thread::sleep(Duration::from_millis(120));
    }
}

/// Apps currently using `capability`.
fn scan(capability: &str) -> Vec<String> {
    let mut out = Vec::new();
    let base = format!("{BASE}{capability}");
    scan_key(&base, false, &mut out);
    scan_key(&format!("{base}\\NonPackaged"), true, &mut out);
    out.sort();
    out.dedup();
    out
}

fn scan_key(path: &str, non_packaged: bool, out: &mut Vec<String>) {
    let Some(h) = open(path) else { return };
    let mut i = 0u32;
    loop {
        let mut name = [0u16; 512];
        let mut len = name.len() as u32;
        let r = unsafe { RegEnumKeyExW(h, i, Some(PWSTR(name.as_mut_ptr())), &mut len, None, None, None, None) };
        if r != WIN32_ERROR(0) {
            break;
        }
        i += 1;
        let sub = from_wide(&name[..len as usize]);
        if sub == "NonPackaged" || (non_packaged && is_own_exe(&sub)) {
            continue;
        }
        let read_q = |value: PCWSTR| -> u64 {
            let mut v: u64 = 0;
            let mut sz = 8u32;
            let subw: Vec<u16> = sub.encode_utf16().chain(Some(0)).collect();
            let r = unsafe {
                RegGetValueW(
                    h,
                    PCWSTR(subw.as_ptr()),
                    value,
                    RRF_RT_REG_QWORD,
                    None,
                    Some(&mut v as *mut u64 as *mut _),
                    Some(&mut sz),
                )
            };
            if r == WIN32_ERROR(0) {
                v
            } else {
                u64::MAX
            }
        };
        let start = read_q(w!("LastUsedTimeStart"));
        let stop = read_q(w!("LastUsedTimeStop"));
        if stop == 0 && start != 0 && start != u64::MAX {
            out.push(app_name(&sub, non_packaged));
        }
    }
    unsafe {
        let _ = RegCloseKey(h);
    }
}

/// Consent-store keys spell paths with '#' instead of '\'.
fn is_own_exe(key: &str) -> bool {
    std::env::current_exe().map_or(false, |p| key.replace('#', "\\").eq_ignore_ascii_case(&p.to_string_lossy()))
}

/// "C:#Program Files#Discord#Discord.exe" → "Discord";
/// "Microsoft.WindowsCamera_8wekyb3d8bbwe" → "WindowsCamera".
fn app_name(key: &str, non_packaged: bool) -> String {
    if non_packaged {
        let file = key.rsplit('#').next().unwrap_or(key);
        let stem = file.rsplit_once('.').map_or(file, |(s, _)| s);
        let mut c = stem.chars();
        return match c.next() {
            Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
            None => key.to_string(),
        };
    }
    let pkg = key.split('_').next().unwrap_or(key);
    pkg.rsplit('.').next().unwrap_or(pkg).to_string()
}

#[cfg(test)]
mod tests {
    use super::app_name;

    #[test]
    fn names() {
        assert_eq!(app_name("C:#Program Files#Discord#app-1.0#Discord.exe", true), "Discord");
        assert_eq!(app_name("Microsoft.WindowsCamera_8wekyb3d8bbwe", false), "WindowsCamera");
    }
}
