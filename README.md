# Dynamic Notch for Windows

A MacBook-style notch / desktop island for Windows, written in Rust.

It sits at the top center of the screen as a small black notch and grows with spring animations: live activities widen it, events "peek" out of it for a few seconds, and hovering or clicking opens the full panel.

## Features

| Module | What it does | How it is wired |
|---|---|---|
| **Media** | Artwork, title/artist, scrub bar, prev / play-pause / next. The compact notch shows the cover plus an audio visualizer tinted from the album art. Peeks on track change. | Windows Global System Media Transport Controls (Spotify, browsers, Media Player, VLC, …) |
| **Timer / Stopwatch** | Presets, ±1 min, pause/resume, ring countdown in the notch, sound and peek when done | ticks once per second, only while running |
| **Downloads** | In-progress browser downloads with size and speed, a "complete" peek, open / show in folder | `ReadDirectoryChangesW` on the Downloads folder (`.crdownload`, `.part`, …) |
| **Mic / Camera** | Green and orange privacy dots in the notch, icons in the header, a peek when an app starts using them | registry change notifications on the Windows capability consent store |
| **Clipboard** | Most recent items (text, links, files, image thumbnails); click to copy again | `AddClipboardFormatListener`; history stays in memory only; items flagged by password managers are skipped |
| **Battery** | Level and charging state in the header; peeks for plugged in, unplugged, low and full | `WM_POWERBROADCAST` |
| **Ask AI** | Quick prompt (`Ctrl+Alt+Space`), streamed answer, follow-ups, copy | OpenRouter API over WinHTTP + SSE |

Other behavior:
- **Secondary activity bubble:** when two things are live (music and a timer, for example), the second one sits in a mini notch to the right.
- **Hover to expand / click / hotkey.** The panel collapses quickly when the mouse leaves. It stays open when opened by hotkey or CLI, until you interact with it.
- **Glass:** real backdrop blur from the Windows compositor, plus a soft shadow.
- **Multi-monitor:** primary, a fixed monitor, or "follow the active window". Per-monitor DPI v2.
- **Fullscreen apps:** the notch hides for games, videos and presentations.
- **System tray:** toggle modules, pick the monitor, blur, autostart, edit settings, quit. The same menu opens on right-click of the notch.

## Build & run

Requirements: Windows 10 1903+ (Windows 11 recommended) and a Rust stable toolchain (MSVC). Both x64 and ARM64 work.

```bash
cargo build --release
```

```bash
target/release/dynamic-notch.exe
```

The release binary is about 1 MB and has no runtime dependencies.

## AI setup (OpenRouter)

The API key is **never** stored in `config.toml`. It comes from `OPENROUTER_API_KEY` in the environment, or from a `.env` file:

```bash
cp .env.example .env
```

Then edit `.env` and set `OPENROUTER_API_KEY=sk-or-v1-...`. Use a regular **API key** from <https://openrouter.ai/settings/keys>. Management keys are rejected with "User not found".

`.env` is listed in `.gitignore`. It is searched next to the exe and up to three parent folders (so `target/release` finds the project's `.env`), in the working directory, and in `%APPDATA%\DynamicNotch\`. The key is read on every request, so editing `.env` doesn't need a restart.

The default model is the cheap and fast `google/gemini-3.1-flash-lite`. To change it, set `[ai] model = "..."` in the config to any OpenRouter model ID.

## Settings

Settings live in `%APPDATA%\DynamicNotch\config.toml`. The file is created on first run with comments, and changes apply as soon as you save. Highlights:

```toml
[general]
monitor = "primary"        # "primary" | "active" | 1, 2, ...
expand_on_hover = true
hover_delay_ms = 140
collapse_delay_ms = 60
idle_style = "notch"       # "notch" | "pill" | "hidden"

[appearance]
blur = true
expanded_opacity = 0.82
bounce = 0.3               # spring overshoot
animation_speed = 1.0

[hotkeys]
toggle = "Ctrl+Alt+N"
ai = "Ctrl+Alt+Space"
timer = "Ctrl+Alt+T"
clipboard = "Ctrl+Alt+V"
play_pause = ""
```

The log is written to `%APPDATA%\DynamicNotch\notch.log`. Set `DN_DEBUG=1` to also log frame statistics.

## Command line / scripting API

Running the exe again while the notch is running forwards the command to the existing instance:

```bash
dynamic-notch notify "Build finished" "All tests passed"
```

```bash
dynamic-notch progress "Export video" 42
```

```bash
dynamic-notch progress "Export video" done
```

```bash
dynamic-notch timer 25
```

```bash
dynamic-notch ask "convert 72F to C"
```

Also available: `toggle`, `expand [page]`, `collapse`, `ai`, `reload`, `quit`.

## Architecture

```
main.rs        single instance, CLI forwarding, DPI awareness, startup
window.rs      Win32 window + wndproc → App (panic-safe, re-entrancy safe)
app.rs         state machine (idle → peek → expanded), springs, layout, input,
               placement (multi-monitor/DPI), hit-test region, fullscreen hiding
host.rs        Windows.UI.Composition visual tree:
                 backdrop (HostBackdropBrush blur, clipped to the notch)
                 content  (2-buffer composition swap chain, Direct2D)
                 bars     (visualizer animated by the compositor, 0 CPU)
pacer.rs       v-sync thread (compositor clock); runs only while animating
gfx/           Direct2D painter, DirectWrite text, vector icons, WIC images
ui.rs          tiny immediate-mode widgets (buttons, progress, cards)
anim.rs        frame-rate independent springs (response + damping ratio)
bus.rs         worker threads → UI thread messages (PostMessage, no polling)
modules/       one file per feature (see below)
sys/           WinHTTP SSE client, .env loader, clipboard, autostart, watchers
```

**Performance model.** Nothing is drawn unless something changes. A v-sync thread wakes only while a spring is moving. Every integration is event-driven: SMTC events, registry, directory and clipboard notifications, and power broadcasts. Measured on an ARM64 laptop (4K at 150 %), release build:

- startup to window: about 0.35 s
- idle CPU: 0 ms per 10 s
- private memory: about 25 MB idle, around 37 MB peak while animating, settling back to about 27–31 MB

Rounded shapes and vector icons are tessellated once and cached on the GPU (`ID2D1GeometryRealization`), and outlines are drawn as filled paths. Re-tessellating every rounded shape each frame made some drivers grow their upload heap by about 64 MB.

### Adding a module

Implement `modules::Module` and add it to `modules::all()`:

```rust
pub struct Weather { temp: Option<f32> }

impl Module for Weather {
    fn id(&self) -> ModuleId { "weather" }
    fn title(&self) -> &str { "Weather" }
    fn icon(&self) -> Icon { Icon::Globe }

    fn start(&mut self, cx: &mut Cx) {
        let bus = cx.bus.clone();
        std::thread::spawn(move || { /* fetch… */ bus.to_module("weather", 21.5f32) });
    }
    fn on_message(&mut self, msg: Box<dyn Any + Send>, cx: &mut Cx) {
        if let Ok(t) = msg.downcast::<f32>() { self.temp = Some(*t); cx.fx.redraw = true; }
    }
    fn card(&self) -> Option<CardSize> { Some(CardSize::Small) }
    fn draw_card(&mut self, ui: &mut Ui, r: Rect) { /* ui.card(...), ui.p.text(...) */ }
}
```

A module can contribute:
- a compact **activity** (left and right slots: image, icon, text, ring, visualizer)
- **indicators** (privacy dots, battery)
- **peeks** (`cx.fx.peek(...)`)
- a **home card**
- a full **page** (tab)

It can also receive keyboard input, system events and CLI commands (`SystemEvent::Command`).

## Known limitations

- Browsers don't expose download progress to other apps, so browser downloads show size and speed with an indeterminate bar. Scripts can report exact progress through `dynamic-notch progress`.
- The visualizer is a stylized animation, not a real audio spectrum. That keeps CPU at zero while music plays.
