# Monitor Switcher GUI — Design

**Date:** 2026-07-31
**Status:** Approved for planning

## 1. Objective

Turn `monitor_switcher` from a config-file-driven tray utility into a tray-first desktop
application with a Dioxus GUI. All configuration currently done by hand-editing
`config.toml` moves into the UI: picking USB devices by friendly name, assigning monitor
inputs by clicking a to-scale monitor layout, and testing an input before committing to it.

The core behaviour is unchanged: when a watched USB device appears or disappears (a KVM
switch being toggled), switch monitor inputs via DDC/CI.

## 2. Decisions

| Decision | Choice | Rationale |
|---|---|---|
| App shape | Tray-first, window opened on demand | Matches use: monitoring runs headless, UI is touched only to configure |
| DDC backend | Keep `ControlMyMonitor.exe` | Proven, already working; rewriting it is not the goal of this project |
| Device detection | Event-driven `WM_DEVICECHANGE` | Existing behaviour; instant, zero idle CPU. **Not** a 2s poll loop |
| Async runtime | None (no tokio) | Nothing to schedule. Event-driven watcher thread + on-demand process spawns |
| USB enumeration | Win32 SetupAPI, drop `hidapi` | `hidapi` cannot see non-HID devices (e.g. a webcam) and yields generic names |
| Monitor view | Read-only to-scale layout | Identifying screens is in scope; rearranging the Windows desktop is not |
| Monitor identity | Serial number | `\\.\DISPLAY1\Monitor0` shuffles across replug/reboot and silently retargets |
| Config migration | Automatic v1 → v2 | An existing working setup must survive the upgrade |
| Styling | Plain CSS with custom properties | Tailwind means npm plus a watcher bolted onto a Rust build, for four tabs |
| Command editing | Removed | Fully replaced by the UI; users never see a `/SetValue` string again |

### Explicitly out of scope for v1

Toast notifications (the monitors visibly switching *is* the notification), draggable
desktop arrangement, raw command-string editing, a mock layer over the Windows FFI.

## 3. Architecture

Single binary, `#![windows_subsystem = "windows"]`. Three threads:

**Watcher thread.** Hidden Win32 `WS_OVERLAPPED` window with a `GetMessageW` pump — the
existing mechanism, preserved. Receives `WM_DEVICECHANGE`, owns the state machine, executes
switches. Runs whether or not a window is open. Emits `Event` values over an `mpsc` channel.

**Main / event-loop thread.** Dioxus desktop (tao/wry). Owns the tray icon, because
`tray-icon` requires the event-loop thread on Windows. Tray menu: Open, Toggle Monitoring,
Quit.

**UI.** A `use_coroutine` drains the channel into Dioxus signals. Closing the window calls
`set_visible(false)` rather than exiting. First launch with no config opens the window;
subsequent launches start hidden.

User-triggered `ControlMyMonitor.exe` invocations run on a short-lived scratch thread so the
UI never blocks.

### Module layout

```
src/main.rs            bootstrap, tray, window lifecycle
src/config.rs          schema, load/save, v1 -> v2 migration
src/watcher.rs         hidden window, state machine, cooldown
src/hardware/usb.rs    SetupAPI enumeration
src/hardware/mon.rs    EnumDisplayMonitors layout + ControlMyMonitor exec/parse
src/deps.rs            ControlMyMonitor detect / download / browse
src/startup.rs         shell:startup shortcut
src/ui/dashboard.rs    status, log
src/ui/devices.rs      USB picker
src/ui/monitors.rs     layout, input rules, test flow
src/ui/settings.rs     paths, startup, cooldown
```

Each hardware module exposes plain functions returning owned data (`Vec<UsbDevice>`,
`Vec<MonitorInfo>`); the UI never touches Win32 or spawns processes directly.

## 4. Configuration

```toml
version = 2
cooldown_secs = 60
monitoring_enabled = true
control_my_monitor_path = ""   # empty = alongside the executable

[[devices]]
id = "VID_046D&PID_085C"
name = "Logitech G502 HERO"    # cached, so unplugged devices still display a name
class = "Mouse"

[[monitors]]
serial = "ABC123456"
label = "Dell U2720Q"
on_connect = 15
on_disconnect = 17
```

Stored next to the executable, as today. Written immediately on every UI change.

The VCP code is hardcoded to 60 (input select). A configurable field would be config for a
value that never varies in this application.

`ControlMyMonitor.exe` accepts a serial number directly as its monitor argument, so serial
works both as the stable config key and as the value passed at execution time.

### Migration

On load, a config without a `version` key is treated as v1:

1. Dump monitor information via `/stext` to resolve `\\.\DISPLAYn\Monitor0` names to serials.
2. Parse each `/SetValue "<monitor>" 60 <value>` string in `connect_cmds` and
   `disconnect_cmds` into `on_connect` / `on_disconnect` entries.
3. Copy `monitored_devices` into `[[devices]]`, resolving names via SetupAPI where the
   device is currently present.
4. Write `config.toml.bak`, then save the v2 file.

Entries whose display name no longer resolves to a serial are retained and shown in the
Monitors view as "unmatched", rather than being silently dropped.

If `ControlMyMonitor.exe` is absent at migration time, step 1 cannot run. In that case the
v1 file is left untouched, defaults are loaded, and the missing-dependency banner is shown;
migration is retried on the next load once the tool is available. A v1 config is never
partially migrated.

## 5. Hardware layer

### USB (`hardware/usb.rs`)

`SetupDiGetClassDevs(DIGCF_PRESENT | DIGCF_ALLCLASSES)`, iterating `SetupDiEnumDeviceInfo`:

- Instance ID yields `VID_xxxx&PID_xxxx` (uppercase hex, 4 digits).
- `SPDRP_FRIENDLYNAME`, falling back to `SPDRP_DEVICEDESC`, yields the display name.
- `SPDRP_CLASS` yields the category used for the list icon (Mouse, Keyboard, Camera,
  HIDClass, USB).

Results are deduplicated by VID&PID — a single physical mouse produces several device nodes,
which is why the raw list is long and mostly unnamed.

Presence detection reuses this exact enumeration, so the picker and the switching logic can
never disagree about what is plugged in.

`hidapi` is removed from `Cargo.toml`.

### Monitors (`hardware/mon.rs`)

Two sources, merged on the `\\.\DISPLAYn` prefix:

- `EnumDisplayMonitors` + `GetMonitorInfoW` — position rectangle, resolution, primary flag,
  GDI device name. This is what the layout is drawn from.
- `ControlMyMonitor.exe /stext <tmpfile>` — serial number, model name, current VCP 60 value.
  Parsed, then the temp file is deleted.

Both run on demand: opening the Monitors view, or pressing Refresh. Never on a timer.

## 6. Watcher state machine

```
on WM_DEVICECHANGE (DBT_DEVNODES_CHANGED | DBT_DEVICEARRIVAL | DBT_DEVICEREMOVECOMPLETE):
    restart a 500ms settle timer      # a KVM toggle fires a burst of events

on settle:
    present = any(configured device id present in enumerate())
    if now < cooldown_until:
        dirty = true
        return
    if present != last_state:
        for each configured monitor:
            set VCP 60 = present ? on_connect : on_disconnect
        last_state = present
        cooldown_until = now + cooldown_secs
        dirty = false

on cooldown expiry:
    if dirty: re-evaluate once        # resync
```

Cooldown defaults to 60 seconds, adjustable in Settings over a 2–300s range. This addresses
observed connect/disconnect flapping.

The re-evaluation at cooldown expiry is required: a plain "ignore events for 60s" would
leave the monitors on the wrong input if the KVM were flipped back during the window. The
accepted tradeoff is that a deliberate flip inside the cooldown is honoured at expiry rather
than immediately.

The state machine is a pure function of `(present: bool, now: Instant, &mut WatcherState)`
returning an optional action. It performs no I/O, so it is unit-testable on any platform.

## 7. User interface

Sidebar navigation, four views. Dark theme via CSS custom properties in a single stylesheet.

### Dashboard

Large status pill (KVM Connected / Disconnected), monitoring on/off toggle, a cooldown
countdown while one is active, and a live event log: timestamp, device name, action,
colour-coded. The log is an in-memory ring buffer of the last 200 entries and is not
persisted.

### Devices

A table of every currently present USB device: class icon, friendly name,
`VID_xxxx&PID_xxxx`, and a toggle to watch it. Devices configured but not currently present
appear greyed at the bottom with their cached name and can be removed. A search box and a
"monitored only" filter are provided. Toggling writes `config.toml` immediately.

### Monitors

Monitor boxes drawn to scale in their real desktop positions, numbered, primary marked, each
showing its current input. The layout is read-only. An Identify button flashes large numbers
on the physical screens.

Selecting a box opens its panel: label, serial, and dropdowns for *KVM connected → input*
and *KVM disconnected → input*.

The input dropdown offers the known DDC codes — VGA 1, DVI-1 3, DVI-2 4, DP-1 15, DP-2 16,
HDMI-1 17, HDMI-2 18 — plus a custom numeric field, because vendors deviate from the
standard.

**Test flow.** Read the monitor's current value, set the candidate value, show an 8-second
countdown, then **automatically revert**. Only after reverting does a dialog appear: "Input
15 (DP-1) — use it for: [KVM connected] [KVM disconnected] [Both] [Discard]".

Reverting before asking is deliberate: while the test is running, the GUI may be displayed
on an input the user can no longer see. Test is disabled while a switch cooldown is active.

### Settings

`ControlMyMonitor.exe` path and status (found / missing, with Download and Browse actions),
Run at startup toggle, cooldown slider, Open config folder, and NirSoft attribution with a
link to the tool's page.

## 8. Error handling

- **ControlMyMonitor missing** — persistent banner, monitoring automatically disabled, and a
  Download action that fetches the NirSoft zip, extracts `ControlMyMonitor.exe` next to the
  application, and verifies it runs. A Browse action locates an existing copy.
- **Command failure** — a red log entry carrying the tool's stderr. Never silent, never a
  panic.
- **Unparseable config** — renamed to `config.toml.broken`, defaults loaded, banner shown
  explaining what happened.
- **Unresolvable monitor serial at switch time** — logged as a warning; remaining monitors
  still switch.

Errors propagate as `Result` throughout. `unwrap` is confined to cases that cannot fail by
construction.

## 9. Testing

Unit tests, runnable under `cargo test` on any platform:

- `parse_vid_pid` and instance-ID extraction
- v1 → v2 config migration, including the unmatched-monitor case
- `/stext` output parsing
- The watcher state machine: transitions, debounce, cooldown suppression, and the
  dirty-resync at expiry

The Windows FFI layer gets no mock abstraction — introducing traits for a single
implementation adds indirection without adding coverage. It is verified by running the
application on the target machine.

## 10. Implementation order

Each step leaves the application in a working, launchable state:

1. Config v2 schema, load/save, migration, and their tests. No UI.
2. SetupAPI enumeration replacing `hidapi`, wired into the existing headless watcher.
3. Watcher extracted into `watcher.rs` with the pure state machine, debounce, and cooldown,
   plus its tests. Behaviour still headless and equivalent to today's.
4. Dioxus shell: window, tray, hide-on-close, event channel, Dashboard view.
5. Devices view.
6. Monitor enumeration and layout rendering, then the input rules and test flow.
7. Settings: dependency detection, download assistant, startup shortcut, cooldown slider.

## 11. Startup integration

The Run at startup toggle creates or removes a `.lnk` shortcut in `shell:startup` via
`IShellLink` COM, replacing the manual instructions currently documented in the README.
