//! User configuration: `%APPDATA%\DynamicNotch\config.toml`.
//!
//! Every field has a default, so partial files are fine.  The file is watched
//! and hot-reloaded on save.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::util::app_dir;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
#[derive(Default)]
pub struct Config {
    pub general: General,
    pub appearance: Appearance,
    pub hotkeys: Hotkeys,
    pub modules: Modules,
    pub media: MediaCfg,
    pub timer: TimerCfg,
    pub clipboard: ClipboardCfg,
    pub downloads: DownloadsCfg,
    pub battery: BatteryCfg,
    pub ai: AiCfg,
    pub locks: LocksCfg,
    pub shelf: ShelfCfg,
    pub ports: PortsCfg,
    pub voice: VoiceCfg,
    pub osd: OsdCfg,
    pub bluetooth: BluetoothCfg,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct General {
    /// "primary", "active" (follows the focused window) or a 1-based monitor number.
    pub monitor: String,
    pub expand_on_hover: bool,
    pub hover_delay_ms: u32,
    pub collapse_delay_ms: u32,
    pub hide_in_fullscreen: bool,
    /// "notch" (always visible), "pill" (thin line when idle) or "hidden" (only on activity/hover).
    pub idle_style: String,
    pub start_with_windows: bool,
}

impl Default for General {
    fn default() -> Self {
        Self {
            monitor: "primary".into(),
            expand_on_hover: true,
            hover_delay_ms: 140,
            collapse_delay_ms: 60,
            hide_in_fullscreen: true,
            idle_style: "notch".into(),
            start_with_windows: false,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Appearance {
    pub notch_width: f32,
    pub notch_height: f32,
    pub expanded_width: f32,
    /// Background opacity when collapsed (1.0 = solid black like a real notch).
    pub collapsed_opacity: f32,
    /// Background opacity when expanded (lower = more glass).
    pub expanded_opacity: f32,
    /// Blur what is behind the expanded notch.
    pub blur: bool,
    pub shadow: bool,
    /// 0 = no overshoot, 1 = very bouncy.
    pub bounce: f32,
    /// Animation speed multiplier (2.0 = twice as fast).
    pub animation_speed: f32,
    /// "auto" (from album art) or "#RRGGBB".
    pub accent: String,
    /// Extra UI scale on top of the monitor DPI.
    pub scale: f32,
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            notch_width: 188.0,
            notch_height: 32.0,
            expanded_width: 600.0,
            collapsed_opacity: 1.0,
            expanded_opacity: 0.82,
            blur: true,
            shadow: false,
            bounce: 0.3,
            animation_speed: 1.0,
            accent: "auto".into(),
            scale: 1.0,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Hotkeys {
    pub toggle: String,
    pub ai: String,
    pub timer: String,
    pub clipboard: String,
    pub play_pause: String,
    /// Voice dictation: tap to start/stop, or hold to talk and release to finish.
    pub dictate: String,
}

impl Default for Hotkeys {
    fn default() -> Self {
        Self {
            toggle: "Ctrl+Alt+N".into(),
            ai: "Ctrl+Alt+Space".into(),
            timer: "Ctrl+Alt+T".into(),
            clipboard: "Ctrl+Alt+V".into(),
            play_pause: String::new(),
            dictate: "Ctrl+Alt+D".into(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Modules {
    pub media: bool,
    pub timer: bool,
    pub downloads: bool,
    pub privacy: bool,
    pub clipboard: bool,
    pub battery: bool,
    pub ai: bool,
    pub voice: bool,
    pub shelf: bool,
    pub ports: bool,
    pub locks: bool,
    pub osd: bool,
    pub bluetooth: bool,
    pub stats: bool,
}

impl Default for Modules {
    fn default() -> Self {
        Self {
            media: true,
            timer: true,
            downloads: true,
            privacy: true,
            clipboard: true,
            battery: true,
            ai: true,
            voice: true,
            shelf: true,
            ports: true,
            locks: true,
            osd: true,
            bluetooth: true,
            stats: true,
        }
    }
}

impl Modules {
    pub fn is_enabled(&self, id: &str) -> bool {
        match id {
            "media" => self.media,
            "timer" => self.timer,
            "downloads" => self.downloads,
            "privacy" => self.privacy,
            "clipboard" => self.clipboard,
            "battery" => self.battery,
            "ai" => self.ai,
            "voice" => self.voice,
            "shelf" => self.shelf,
            "ports" => self.ports,
            "locks" => self.locks,
            "osd" => self.osd,
            "bluetooth" => self.bluetooth,
            "stats" => self.stats,
            _ => true,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct MediaCfg {
    pub visualizer: bool,
    /// Show a peek when the track changes.
    pub peek_on_track_change: bool,
    /// Keep the live activity this many seconds after playback pauses.
    pub linger_after_pause_secs: u32,
}

impl Default for MediaCfg {
    fn default() -> Self {
        Self { visualizer: true, peek_on_track_change: true, linger_after_pause_secs: 20 }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct TimerCfg {
    pub presets_min: Vec<u32>,
    pub sound: bool,
    /// Gain applied to the "done" chime (1.0 = as recorded).
    pub sound_volume: f32,
}

impl Default for TimerCfg {
    fn default() -> Self {
        Self { presets_min: vec![1, 3, 5, 10, 15, 25, 45], sound: true, sound_volume: 1.6 }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ClipboardCfg {
    pub history: usize,
    pub flash_on_copy: bool,
}

impl Default for ClipboardCfg {
    fn default() -> Self {
        Self { history: 8, flash_on_copy: true }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct DownloadsCfg {
    /// Extra folders to watch (the user's Downloads folder is always watched).
    pub folders: Vec<String>,
    pub peek_on_complete: bool,
}

impl Default for DownloadsCfg {
    fn default() -> Self {
        Self { folders: Vec::new(), peek_on_complete: true }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct BatteryCfg {
    pub low_threshold: u8,
    pub peek_on_plug: bool,
}

impl Default for BatteryCfg {
    fn default() -> Self {
        Self { low_threshold: 20, peek_on_plug: true }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct AiCfg {
    /// Any OpenRouter model id. The API key is never stored here: it is read
    /// from OPENROUTER_API_KEY (environment or a git-ignored `.env` file).
    pub model: String,
    /// OpenAI-compatible chat completions endpoint.
    pub endpoint: String,
    pub max_tokens: u32,
    pub system_prompt: String,
}

impl Default for AiCfg {
    fn default() -> Self {
        Self {
            model: "google/gemini-3.1-flash-lite".into(),
            endpoint: "https://openrouter.ai/api/v1/chat/completions".into(),
            max_tokens: 4096,
            system_prompt: "You are a quick assistant living in a small desktop notch. \
                Answer concisely: prefer a few short sentences or a compact list. Plain text, no markdown headings."
                .into(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct LocksCfg {
    /// Peek when Caps Lock / Num Lock / Scroll Lock is toggled.
    pub caps: bool,
    pub num: bool,
    pub scroll: bool,
}

impl Default for LocksCfg {
    fn default() -> Self {
        Self { caps: true, num: true, scroll: false }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ShelfCfg {
    /// Keep parked files across restarts (only the paths are stored).
    pub remember: bool,
}

impl Default for ShelfCfg {
    fn default() -> Self {
        Self { remember: true }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct PortsCfg {
    /// Peek when a dev server starts listening.
    pub peek_on_start: bool,
    /// Always show these ports, whatever process owns them.
    pub extra_ports: Vec<u16>,
    /// Never show these ports.
    pub ignore_ports: Vec<u16>,
    /// Extra process names that count as dev servers (e.g. "myserver.exe").
    pub extra_processes: Vec<String>,
}

impl Default for PortsCfg {
    fn default() -> Self {
        Self { peek_on_start: true, extra_ports: Vec::new(), ignore_ports: Vec::new(), extra_processes: Vec::new() }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct VoiceCfg {
    /// Groq speech-to-text model. The API key is never stored here: it is read
    /// from GROQ_API_KEY (environment or a git-ignored `.env` file).
    pub model: String,
    /// OpenAI-compatible API base URL.
    pub endpoint: String,
    /// "" = detect automatically, or an ISO code like "en" / "de".
    pub language: String,
    /// Names and terms the transcription should spell correctly.
    pub vocabulary: Vec<String>,
    /// LLM pass after transcription: "light" (fillers, punctuation),
    /// "medium" (also light rephrasing) or "off".
    pub cleanup: String,
    pub cleanup_model: String,
    /// Paste into the focused app; otherwise the text is only copied.
    pub auto_paste: bool,
    /// Recording stops automatically after this long.
    pub max_seconds: u32,
}

impl Default for VoiceCfg {
    fn default() -> Self {
        Self {
            model: "whisper-large-v3-turbo".into(),
            endpoint: "https://api.groq.com/openai/v1".into(),
            language: String::new(),
            vocabulary: Vec::new(),
            cleanup: "light".into(),
            cleanup_model: "llama-3.1-8b-instant".into(),
            auto_paste: true,
            max_seconds: 300,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct OsdCfg {
    /// Peek when the output volume or mute state changes.
    pub volume: bool,
    /// Handle the volume keys ourselves so Windows' own flyout stays hidden
    /// (falls back to the Windows flyout while the notch is hidden or open).
    pub replace_system_flyout: bool,
    /// Volume change per key press, in percent.
    pub volume_step: u32,
    /// Peek when the built-in display's brightness changes.
    pub brightness: bool,
}

impl Default for OsdCfg {
    fn default() -> Self {
        Self { volume: true, replace_system_flyout: true, volume_step: 2, brightness: true }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct BluetoothCfg {
    pub peek_on_connect: bool,
    pub peek_on_disconnect: bool,
    /// Warn once when a connected device's battery drops to this percentage.
    pub low_battery: u8,
}

impl Default for BluetoothCfg {
    fn default() -> Self {
        Self { peek_on_connect: true, peek_on_disconnect: true, low_battery: 15 }
    }
}

pub fn config_path() -> PathBuf {
    app_dir().join("config.toml")
}

// returned once per load and unpacked right away; boxing would buy nothing
#[allow(clippy::large_enum_variant)]
pub enum LoadResult {
    Ok(Config),
    /// File had errors; defaults (or last good config) should be kept.
    Invalid(String),
}

pub fn load() -> LoadResult {
    let path = config_path();
    match std::fs::read_to_string(&path) {
        Ok(text) => match toml::from_str::<Config>(&text) {
            Ok(c) => LoadResult::Ok(c.sanitized()),
            Err(e) => LoadResult::Invalid(e.message().to_string()),
        },
        Err(_) => {
            let _ = std::fs::write(&path, DEFAULT_TEMPLATE);
            LoadResult::Ok(Config::default())
        }
    }
}

/// Persist a single `[section] key = value` change while keeping the user's
/// comments and formatting intact.
pub fn set_value(section: &str, key: &str, value: &str) {
    let path = config_path();
    let text = std::fs::read_to_string(&path).unwrap_or_else(|_| DEFAULT_TEMPLATE.to_string());
    let mut out = Vec::new();
    let mut in_section = false;
    let mut done = false;
    let mut section_seen = false;
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with('[') && t.ends_with(']') {
            if in_section && !done {
                out.push(format!("{key} = {value}"));
                done = true;
            }
            in_section = &t[1..t.len() - 1] == section;
            section_seen |= in_section;
        } else if in_section && !done {
            let k = t.split('=').next().unwrap_or("").trim();
            if k == key && !t.starts_with('#') {
                out.push(format!("{key} = {value}"));
                done = true;
                continue;
            }
        }
        out.push(line.to_string());
    }
    if !done {
        if !section_seen {
            out.push(String::new());
            out.push(format!("[{section}]"));
        }
        out.push(format!("{key} = {value}"));
    }
    let _ = std::fs::write(&path, out.join("\n") + "\n");
}

impl Config {
    fn sanitized(mut self) -> Self {
        let a = &mut self.appearance;
        a.notch_width = a.notch_width.clamp(80.0, 600.0);
        a.notch_height = a.notch_height.clamp(18.0, 64.0);
        a.expanded_width = a.expanded_width.clamp(420.0, 1000.0);
        a.collapsed_opacity = a.collapsed_opacity.clamp(0.2, 1.0);
        a.expanded_opacity = a.expanded_opacity.clamp(0.2, 1.0);
        a.bounce = a.bounce.clamp(0.0, 1.0);
        a.animation_speed = a.animation_speed.clamp(0.25, 4.0);
        a.scale = a.scale.clamp(0.5, 2.5);
        self.clipboard.history = self.clipboard.history.clamp(1, 30);
        self.voice.max_seconds = self.voice.max_seconds.clamp(5, 780);
        self.osd.volume_step = self.osd.volume_step.clamp(1, 25);
        self.bluetooth.low_battery = self.bluetooth.low_battery.min(100);
        self
    }
}

pub const DEFAULT_TEMPLATE: &str = r##"# Dynamic Notch configuration
# Saved changes are applied immediately (hot reload).

[general]
# "primary", "active" (follow the focused window) or a monitor number (1, 2, ...)
monitor = "primary"
expand_on_hover = true
hover_delay_ms = 140
collapse_delay_ms = 60
hide_in_fullscreen = true
# "notch" = always visible, "pill" = slim line when idle, "hidden" = only on activity/hover
idle_style = "notch"
start_with_windows = false

[appearance]
notch_width = 188
notch_height = 32
expanded_width = 600
collapsed_opacity = 1.0
expanded_opacity = 0.82
blur = true
shadow = false
# 0 = no overshoot ... 1 = very bouncy
bounce = 0.3
animation_speed = 1.0
# "auto" picks the accent from album art, or use "#RRGGBB"
accent = "auto"
scale = 1.0

[hotkeys]
# Modifiers: Ctrl, Alt, Shift, Win. Keys: A-Z, 0-9, F1-F24, Space, Enter, Tab, Esc,
# Up/Down/Left/Right, Home, End, PageUp, PageDown, Insert, Delete, `, -, =, [, ], ;, ', ",", ., /
# Leave empty to disable.
toggle = "Ctrl+Alt+N"
ai = "Ctrl+Alt+Space"
timer = "Ctrl+Alt+T"
clipboard = "Ctrl+Alt+V"
play_pause = ""
# Voice dictation: tap to start/stop, or hold to talk and release to finish.
dictate = "Ctrl+Alt+D"

[modules]
media = true
timer = true
downloads = true
privacy = true
clipboard = true
battery = true
ai = true
voice = true
shelf = true
ports = true
locks = true
osd = true
bluetooth = true
stats = true

[media]
visualizer = true
peek_on_track_change = true
linger_after_pause_secs = 20

[timer]
presets_min = [1, 3, 5, 10, 15, 25, 45]
sound = true
# loudness of the "done" chime; 1.0 = system default, capped before it would clip
sound_volume = 1.6

[clipboard]
history = 8
flash_on_copy = true

[downloads]
# Extra folders to watch; your Downloads folder is always watched.
folders = []
peek_on_complete = true

[battery]
low_threshold = 20
peek_on_plug = true

[ai]
# The OpenRouter API key is NOT stored here. Put it in a .env file:
#   OPENROUTER_API_KEY=sk-or-...
# (searched next to the exe, in the project folder and in %APPDATA%\DynamicNotch)
# Any OpenRouter model id. Cheap & fast default; alternatives:
# "deepseek/deepseek-v4.1-flash" (cheapest), "anthropic/claude-haiku-4.5", "anthropic/claude-sonnet-5.5"
model = "google/gemini-3.1-flash-lite"
endpoint = "https://openrouter.ai/api/v1/chat/completions"
max_tokens = 4096
system_prompt = "You are a quick assistant living in a small desktop notch. Answer concisely: prefer a few short sentences or a compact list. Plain text, no markdown headings."

[voice]
# Speech to text with Groq. The API key is NOT stored here. Put it in a .env file:
#   GROQ_API_KEY=gsk_...
# (same places as OPENROUTER_API_KEY). Get one at https://console.groq.com/keys
model = "whisper-large-v3-turbo"
endpoint = "https://api.groq.com/openai/v1"
# "" = detect the language automatically, or e.g. "en", "de"
language = ""
# Names and terms to spell correctly, e.g. ["Groq", "DynamicNotch"]
vocabulary = []
# Clean-up pass after transcription: "light" (fillers, punctuation), "medium" (also light rephrasing) or "off"
cleanup = "light"
cleanup_model = "llama-3.1-8b-instant"
# Paste into the focused app; when false the text is only copied
auto_paste = true
max_seconds = 300

[locks]
# Peek when a lock key is toggled
caps = true
num = true
scroll = false

[shelf]
# Keep parked files across restarts (only the paths are stored)
remember = true

[ports]
# Peek when a dev server starts listening
peek_on_start = true
# Always show / never show these ports
extra_ports = []
ignore_ports = []
# Extra process names that count as dev servers, e.g. ["myserver.exe"]
extra_processes = []

[osd]
# Volume / brightness peeks
volume = true
# Take over the volume keys so the Windows flyout stays hidden
# (the Windows flyout still shows while the notch is hidden or open)
replace_system_flyout = true
# Percent per volume key press
volume_step = 2
# Built-in display only; Windows may still show its own brightness flyout
brightness = true

[bluetooth]
peek_on_connect = true
peek_on_disconnect = true
# Warn once when a connected device's battery drops to this percentage (0 = off)
low_battery = 15
"##;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_parses_to_defaults() {
        let c: Config = toml::from_str(DEFAULT_TEMPLATE).expect("template must parse");
        let d = Config::default();
        assert_eq!(c.general.monitor, d.general.monitor);
        assert_eq!(c.appearance.notch_width, d.appearance.notch_width);
        assert_eq!(c.hotkeys.ai, d.hotkeys.ai);
        assert_eq!(c.ai.model, d.ai.model);
        assert_eq!(c.timer.presets_min, d.timer.presets_min);
        assert_eq!(c.hotkeys.dictate, d.hotkeys.dictate);
        assert_eq!(c.voice.model, d.voice.model);
        assert_eq!(c.voice.cleanup, d.voice.cleanup);
        assert_eq!(c.locks.caps, d.locks.caps);
        assert_eq!(c.ports.peek_on_start, d.ports.peek_on_start);
        assert!(c.modules.voice && c.modules.shelf && c.modules.ports && c.modules.locks);
        assert!(c.modules.osd && c.modules.bluetooth && c.modules.stats);
        assert_eq!(c.osd.volume_step, d.osd.volume_step);
        assert_eq!(c.osd.replace_system_flyout, d.osd.replace_system_flyout);
        assert_eq!(c.bluetooth.low_battery, d.bluetooth.low_battery);
    }

    #[test]
    fn partial_config() {
        let c: Config = toml::from_str("[appearance]\nblur = false\n").unwrap();
        assert!(!c.appearance.blur);
        assert_eq!(c.general.monitor, "primary");
    }
}
