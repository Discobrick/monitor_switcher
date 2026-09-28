# Monitor Switcher

Monitor Switcher is a Windows tray app that switches your monitors' inputs when a USB switch
or KVM is toggled. It watches for the USB devices that move with your KVM (keyboard, mouse,
webcam…) and, when they appear or disappear, tells each monitor which input to show over
DDC/CI.

## Setup

For a step-by-step walkthrough with screenshots, see **[docs/SETUP.md](docs/SETUP.md)**.

1. Run `monitor_switcher.exe`. On first launch it downloads `ControlMyMonitor.exe` from
   NirSoft into the same folder (progress shows under **Dashboard → Activity**). If that
   fails, for example offline, it retries next launch, or use **Settings** to download it or
   point at an existing copy.
2. **Devices** → tick the devices that move with your KVM. Not sure which is which? Click
   **Detect**, toggle the KVM, and the devices that came or went are listed. Click any
   device's name to give it a label; generic names like "USB Input Device" are common.
3. **Monitors** → click each screen and choose the input it should show when the KVM is
   connected and when it is not. The dropdowns list the inputs the monitor itself reports.
   Use **Test** if you're unsure which input is which: the monitor switches, switches back
   on its own after the test duration (15 s by default, set in **Settings**), and only then
   asks whether to keep the value. During a test you can add time or switch back early.
4. **Settings** → optionally enable **Run at startup**.
5. Close the window. The app keeps running in the tray; quit from the tray menu.

Settings are saved to `config.toml` next to the executable as you change them. Configs from
the pre-GUI version (`monitored_devices` / `connect_cmds`) are not migrated: such a file is
renamed to `config.toml.broken` and the app starts fresh.

### Cooldown

After a switch, further switches are held off for the cooldown (60 s by default, set in
**Settings**). If the KVM changes during the cooldown, the monitors are resynced when it ends.

## ControlMyMonitor

Monitor control goes through [ControlMyMonitor](https://www.nirsoft.net/utils/control_my_monitor.html),
a free utility by NirSoft. It is downloaded from NirSoft's own site at first launch rather
than bundled with this app.

## Development

Windows only. `cargo run` starts the app; `cargo test` covers the config, the ControlMyMonitor
output parsers, the zip extraction and the watcher state machine.

A mock hardware backend with a **Debug** tab exists for non-Windows targets
(`src/hardware/mock.rs`, `src/ui/debug.rs`), but it is not built in CI and may be stale.
