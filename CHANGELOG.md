# Changelog

All notable changes to Dynamic Notch are listed here.
The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses [Semantic Versioning](https://semver.org/).

## [Unreleased]

## [0.1.0] - 2026-10-08

The first public release.

### Notch

- A MacBook-style notch at the top of the screen that rests, shows live activities, peeks on events and opens into a frosted-glass panel.
- Spring animations, hover lean, fast collapse when the mouse leaves.
- Hides automatically for fullscreen games, videos and presentations.
- Idle styles: always-visible notch, slim pill, or hidden until something happens.
- Primary monitor, a fixed monitor, or follow the active window.
- Per-monitor DPI aware; renders with Direct2D on a composition swap chain and draws nothing while idle.

### Modules

- **Media**: artwork, title and controls for anything using the Windows media controls, with a compositor-driven visualizer tinted from the album art.
- **Timer**: timers and a stopwatch with a countdown ring in the notch and a chime when done.
- **Downloads**: browser downloads with size and speed, and a peek when they finish.
- **Clipboard**: in-memory history of text, links, files and images; content flagged by password managers is skipped.
- **Shelf**: drop files on the notch to park them, drag them out later.
- **Dictation**: hold or tap `Ctrl+Alt+D`, speak, and the text is pasted into the app you were using (Whisper on Groq, with an optional clean-up pass).
- **Ask AI**: a quick prompt on `Ctrl+Alt+Space` with streamed answers and follow-ups, through any OpenRouter model.
- **Mic and camera**: an indicator and a peek naming the app whenever the microphone or camera is in use.
- **Ports**: local dev servers as they start listening, each with an Open button.
- **Lock keys**: peeks for Caps Lock, Num Lock and Scroll Lock.
- **Battery**: level and charging state, with peeks for plugged in, unplugged, low and full.
- **Volume and brightness**: the notch replaces the Windows volume flyout; brightness changes on the built-in display peek too.
- **Bluetooth**: connect and disconnect peeks with the device's battery level, and a low-battery warning.
- **System stats**: CPU, RAM and GPU load on the home page, sampled only while the notch is open.

### Integrations

- Command-line API: `notify`, `progress`, `timer`, `ask`, `toggle`, `expand`, `collapse`, `ai`, `reload` and `quit` are forwarded to the running notch.
- Claude Code hook bridge: `dynamic-notch claude` peeks when Claude finishes or needs you and brings its terminal back on click.
- Tray icon with modules, monitor, display options and autostart.
- `%APPDATA%\DynamicNotch\config.toml` with hot reload; API keys are read only from the environment or a git-ignored `.env`.

[Unreleased]: https://github.com/eigenes/DynamicNotch/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/eigenes/DynamicNotch/releases/tag/v0.1.0
