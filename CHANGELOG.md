# Changelog

All notable changes to Dynamic Notch are listed here.
The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses [Semantic Versioning](https://semver.org/).

## [Unreleased]

## [0.2.0] - 2026-11-27

### Added

- **Installer**: each release now has a `windows-setup.exe` next to the zips. One file for x64 and ARM64, per-user (no admin), with a Start menu entry and an uninstaller. Running a newer one closes the notch, updates it in place and starts it again. The zips stay for portable use.
- **App icon**: `dynamic-notch.exe`, the installer, the shortcuts and the Apps & features entry now have their own icon.

### Changed

- **Ask AI** now uses the Gemini API (Google AI Studio, free tier works) by default instead of OpenRouter. Put `GEMINI_API_KEY` in your `.env`; the default model is `gemini-3.5-flash-lite`.
- **OpenRouter still works.** If your config points at OpenRouter and `OPENROUTER_API_KEY` is set, nothing changes. Only an OpenRouter config without that key is moved to the Gemini endpoint and model on start.

### Fixed

- **Start with Windows** now follows the exe when it moves, for example from an unzipped copy to the installed one, instead of launching the old copy.
- **The panel no longer gets stuck open** when Windows drops a mouse-leave, focus-loss or drag-leave notification. While something holds the panel open, the notch now re-checks hover, keyboard focus and drag state and closes as expected.

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

[Unreleased]: https://github.com/eigenes/DynamicNotch/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/eigenes/DynamicNotch/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/eigenes/DynamicNotch/releases/tag/v0.1.0
