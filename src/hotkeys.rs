//! Global hotkeys from config strings like "Ctrl+Alt+Space".

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Input::KeyboardAndMouse::*;

use crate::config::Hotkeys as HotkeyCfg;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Toggle,
    Ai,
    Timer,
    Clipboard,
    PlayPause,
    Dictate,
}

const BASE_ID: i32 = 0x4E00;

pub struct Registered {
    pub id: i32,
    pub action: Action,
}

pub fn parse(s: &str) -> Option<(HOT_KEY_MODIFIERS, u32)> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let mut mods = MOD_NOREPEAT;
    let mut vk: Option<u32> = None;
    // split on '+' but allow "+" itself as the final key ("Ctrl+Shift++" is unusual; skip)
    for part in s.split('+').map(str::trim).filter(|p| !p.is_empty()) {
        let lower = part.to_ascii_lowercase();
        match lower.as_str() {
            "ctrl" | "control" => mods |= MOD_CONTROL,
            "alt" => mods |= MOD_ALT,
            "shift" => mods |= MOD_SHIFT,
            "win" | "super" | "meta" => mods |= MOD_WIN,
            _ => vk = Some(key_code(&lower)?),
        }
    }
    vk.map(|v| (mods, v))
}

fn key_code(k: &str) -> Option<u32> {
    let named = match k {
        "space" => VK_SPACE,
        "enter" | "return" => VK_RETURN,
        "tab" => VK_TAB,
        "esc" | "escape" => VK_ESCAPE,
        "backspace" => VK_BACK,
        "up" => VK_UP,
        "down" => VK_DOWN,
        "left" => VK_LEFT,
        "right" => VK_RIGHT,
        "home" => VK_HOME,
        "end" => VK_END,
        "pageup" | "pgup" => VK_PRIOR,
        "pagedown" | "pgdn" => VK_NEXT,
        "insert" | "ins" => VK_INSERT,
        "delete" | "del" => VK_DELETE,
        "`" | "backtick" => VK_OEM_3,
        "-" | "minus" => VK_OEM_MINUS,
        "=" | "equals" => VK_OEM_PLUS,
        "[" => VK_OEM_4,
        "]" => VK_OEM_6,
        ";" => VK_OEM_1,
        "'" => VK_OEM_7,
        "," | "comma" => VK_OEM_COMMA,
        "." | "period" => VK_OEM_PERIOD,
        "/" | "slash" => VK_OEM_2,
        "\\" | "backslash" => VK_OEM_5,
        "playpause" | "mediaplaypause" => VK_MEDIA_PLAY_PAUSE,
        _ => VIRTUAL_KEY(0),
    };
    if named.0 != 0 {
        return Some(named.0 as u32);
    }
    let b = k.as_bytes();
    if b.len() == 1 && (b[0].is_ascii_alphanumeric()) {
        return Some(b[0].to_ascii_uppercase() as u32);
    }
    if let Some(n) = k.strip_prefix('f').and_then(|n| n.parse::<u32>().ok()) {
        if (1..=24).contains(&n) {
            return Some(VK_F1.0 as u32 + n - 1);
        }
    }
    None
}

/// Register all configured hotkeys. Returns what succeeded and a list of
/// human-readable failures (bad syntax or already taken by another app).
pub fn register_all(hwnd: HWND, cfg: &HotkeyCfg) -> (Vec<Registered>, Vec<String>) {
    unregister_all(hwnd);
    let mut ok = Vec::new();
    let mut errors = Vec::new();
    let list = [
        (Action::Toggle, &cfg.toggle),
        (Action::Ai, &cfg.ai),
        (Action::Timer, &cfg.timer),
        (Action::Clipboard, &cfg.clipboard),
        (Action::PlayPause, &cfg.play_pause),
        (Action::Dictate, &cfg.dictate),
    ];
    for (i, (action, s)) in list.iter().enumerate() {
        if s.trim().is_empty() {
            continue;
        }
        match parse(s) {
            Some((mods, vk)) => {
                let id = BASE_ID + i as i32;
                if unsafe { RegisterHotKey(Some(hwnd), id, mods, vk) }.is_ok() {
                    ok.push(Registered { id, action: *action });
                } else {
                    errors.push(format!("{s} is already in use"));
                }
            }
            None => errors.push(format!("can't parse hotkey \"{s}\"")),
        }
    }
    (ok, errors)
}

pub fn unregister_all(hwnd: HWND) {
    for i in 0..8 {
        unsafe {
            let _ = UnregisterHotKey(Some(hwnd), BASE_ID + i);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses() {
        let (m, k) = parse("Ctrl+Alt+Space").unwrap();
        assert!(m.contains(MOD_CONTROL) && m.contains(MOD_ALT));
        assert_eq!(k, VK_SPACE.0 as u32);
        assert_eq!(parse("win+shift+n").unwrap().1, b'N' as u32);
        assert_eq!(parse("Alt+F12").unwrap().1, VK_F12.0 as u32);
        assert!(parse("Ctrl+Nope").is_none());
        assert!(parse("").is_none());
    }
}
