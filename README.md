# nocaps

`nocaps` turns Caps Lock into a configurable keyboard layer on Windows, macOS, and Linux. Hold Caps Lock and press a bound physical key to invoke an action such as an arrow key, Control, volume control, or media playback.

Right-click the struck-through graduation-cap icon and choose **Configure**. The window shows every available action in the same interface on each OS. Click an action's binding, press the physical key that should trigger it, and the change is compiled, applied, and saved immediately. Assigning an already-used key moves that key to the new action.

Remapping defaults to enabled. The tray menu shows **Enabled** or **Disabled** and toggles the state when clicked. The window uses the same state with a **No Caps** or **Caps** button.

The configuration window is implemented in `src/app.rs`. `src/config.rs` contains the JSON model, validation, and compiled runtime lookup; it does not create a window.

## Configuration file

Configuration is loaded at startup and saved as formatted JSON:

- Windows: `%APPDATA%\nocaps\config.json`
- macOS: `~/Library/Application Support/nocaps/config.json`
- Linux: `~/.config/nocaps/config.json`

The repository's `config/default.json` is compiled into the executable. It is not a runtime sidecar file. If the user configuration does not exist, `nocaps` loads the embedded original bindings, writes an editable user copy, and uses them immediately. **Restore defaults** uses the same embedded data.

Example:

```json
{
  "version": 1,
  "enabled": true,
  "bindings": {
    "left_control": "a",
    "left_shift": "s",
    "arrow_up": "i",
    "arrow_down": "k",
    "volume_up": "w",
    "volume_down": "q",
    "volume_mute": "tab"
  }
}
```

Each action can have one key and each key can have one action. Unknown versions and duplicate key assignments are rejected with an explicit error.

## Performance model

JSON and validation run only at startup or when a binding changes. Valid bindings are compiled into a fixed-size array indexed by physical key and published with an atomic pointer swap. Keyboard hooks perform no JSON parsing, hash lookups, linear searches, allocations, or configuration locks. Platform modules only translate native input codes to the shared physical-key enum and shared actions back to native output codes.

Remapping input processing never runs on the renderer thread. Windows installs the low-level hook on a dedicated `THREAD_PRIORITY_HIGHEST` Win32 message-loop thread. Linux keyboard-device workers and the macOS capture worker request realtime/high scheduling priority and continue at normal priority with a warning if the OS denies that request. Linux realtime priority generally requires `CAP_SYS_NICE` or an equivalent service limit.

## Platform setup

### Windows

No additional runtime setup is required. To launch at sign-in, place `nocaps.exe` in the Startup folder (`Win+R`, then `shell:startup`).

### macOS

Grant `nocaps` access in **System Settings → Privacy & Security → Accessibility**.

### Linux

The Linux backend uses evdev and uinput, independently of X11 or Wayland. The user needs read access to keyboard devices under `/dev/input` and write access to `/dev/uinput`. Be aware that membership in the `input` group grants access to raw keyboard input.

Ubuntu/Debian build dependencies:

```bash
sudo apt install libgtk-3-dev libayatana-appindicator3-dev
```

Typical device setup:

```bash
sudo modprobe uinput
sudo usermod -aG input "$USER"
```

Log out and back in after changing group membership. A newly connected keyboard currently requires restarting `nocaps`.

## Building

```bash
cargo test
cargo build --release
```

The executable is `target/release/nocaps` (`nocaps.exe` on Windows). CI builds and tests Windows, macOS, and Linux.
