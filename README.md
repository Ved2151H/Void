# Void

Void is a Windows 11 taskbar replacement. It hides the native taskbar and adds a floating dock at the bottom of the screen plus a notch at the top that adapts to what you are doing.

## Features

- **Dock** — pin apps, drag to reorder, hover for window previews, fixed/smart/peek visibility modes, dock extras (drives, folders, Recycle Bin)
- **Notch** — music player with audio visualizer, Command Center (WiFi, Bluetooth, volume, brightness), status (battery, weather), calendar with Pomodoro timer
- **Overlays** — custom volume and brightness HUDs replace the native Windows on-screen display
- **Settings** — full settings window with themes (dark/light/custom/adaptive), UI scale, per-app mixer, sound export/import
- **Updates** — built-in auto-updater
- **Tray** — restart or quit cleanly; quitting restores the native taskbar

## Getting it

Download the latest installer from the [Releases](https://github.com/Ved2151H/Void/releases/latest) page and run `Void_x.y.z_x64-setup.exe`.

## Building from source

Requirements: [Bun](https://bun.sh/), [Rust stable](https://rustup.rs/) and Visual Studio C++ build tools (Windows 10 or 11).

```bash
git clone https://github.com/Ved2151H/Void.git
cd Void
bun install
bun run tauri dev     # run it in development
bun run build:setup   # build the installer (lands in this folder)
```

## How it works

A Rust backend on Tauri v2 speaks directly to the Windows shell: AppBar registration, global keyboard hooks, WASAPI audio capture, Windows Media Session COM and WMI hardware monitoring. Four webview windows handle the notch, dock, overlays and settings.
