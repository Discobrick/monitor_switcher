# Monitor Switcher GUI Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn `monitor_switcher` from a hand-edited-TOML tray utility into a tray-first Dioxus desktop app where USB devices, monitors, and input rules are all configured through the UI.

**Architecture:** Single Windows binary. A watcher thread keeps the existing hidden-window `WM_DEVICECHANGE` pump and owns a pure state machine (debounce → cooldown → switch). The Dioxus event-loop thread owns the tray icon and a window that hides rather than exits. They communicate over one `mpsc` channel. All hardware access sits behind `src/hardware/mod.rs`, which re-exports either the real Win32 backend or a mock backend chosen by target — so the whole UI runs on Linux.

**Tech Stack:** Rust 2024 edition, Dioxus 0.7 (desktop), `windows` 0.52, `tray-icon` 0.14, `serde` + `toml`, plain CSS. `hidapi` is removed. `ureq` + `zip` added for the dependency downloader.

**Design spec:** `docs/superpowers/specs/2026-07-31-monitor-switcher-gui-design.md`

## Global Constraints

- Target platform is Windows. Development machine is Linux; every task must leave `cargo check` and `cargo test` passing on **both** Linux and Windows.
- `windows` crate stays at **0.52** — do not upgrade to 0.6x, the API churn buys nothing here.
- Dioxus pinned to **0.7.10** (0.8 is alpha). If any Dioxus API in this plan does not compile, check the 0.7 docs and adapt — the plan's structure, not its exact call syntax, is what matters.
- Hardware backend selection is by `#[cfg(windows)]` only. **Never** a Cargo feature, never a runtime flag.
- Modules live in `src/lib.rs` as `pub mod`; `src/main.rs` is a thin binary that consumes them via `use monitor_switcher::…`. No module is ever declared in both.
- Windows-only crates (`windows`, `tray-icon`) live under `[target.'cfg(windows)'.dependencies]`, so a Linux build never pulls them.
- No `tokio`. No polling loops. Device detection is event-driven; everything else is user-triggered.
- VCP code is hardcoded to `60` (input select). Do not add a configurable VCP field.
- `unwrap()`/`expect()` only where failure is impossible by construction. Everything fallible returns `Result`.
- Monitors are keyed by **serial number**, never by `\\.\DISPLAYn\Monitor0`.
- The event log is in-memory only (ring buffer, 200 entries). Never written to disk.
- Deliberate simplifications get a `// ponytail:` comment naming the ceiling.
- Commit after every task. Conventional-commit prefixes (`feat:`, `refactor:`, `test:`, `chore:`).

## File Structure

| File | Responsibility |
|---|---|
| `src/lib.rs` | Module root: `pub mod` declarations for everything below |
| `src/main.rs` | Thin binary: bootstrap, tray icon, window lifecycle, channel wiring |
| `src/config.rs` | v2 schema, load/save, v1→v2 migration |
| `src/watcher.rs` | Pure state machine + the thread that drives it |
| `src/hardware/mod.rs` | Shared types; re-exports real or mock backend by target |
| `src/hardware/parse.rs` | `/stext` parser + VID/PID parsing (pure, cross-platform) |
| `src/hardware/usb.rs` | SetupAPI enumeration (windows only) |
| `src/hardware/mon.rs` | `EnumDisplayMonitors` + ControlMyMonitor exec (windows only) |
| `src/hardware/mock.rs` | Canned devices/monitors (non-windows only) |
| `src/deps.rs` | ControlMyMonitor detect / download / browse |
| `src/startup.rs` | `shell:startup` shortcut (windows), no-op stub (non-windows) |
| `src/ui/mod.rs` | App root, sidebar, shared state, `Event` → signal plumbing |
| `src/ui/dashboard.rs` | Status pill, monitoring toggle, event log |
| `src/ui/devices.rs` | USB picker |
| `src/ui/monitors.rs` | Layout diagram, input rules, test flow |
| `src/ui/settings.rs` | Paths, download assistant, startup, cooldown |
| `src/ui/debug.rs` | Simulation panel (non-windows only) |
| `assets/style.css` | Dark theme, CSS custom properties |

---

## Task 1: Config v2 schema, load and save

**Files:**
- Create: `src/config.rs`
- Modify: `Cargo.toml`

**Interfaces:**
- Consumes: nothing
- Produces: `Config`, `DeviceEntry`, `MonitorRule`, `Config::load(dir: &Path) -> Result<Config, ConfigError>`, `Config::save(&self, dir: &Path) -> Result<(), ConfigError>`, `Config::default()`, `ConfigError`

- [ ] **Step 1: Add serde/toml deps and a `thiserror` dep**

In `Cargo.toml`, under `[dependencies]`, ensure these exist (leave the existing `windows` and `tray-icon` entries alone for now):

```toml
serde = { version = "1.0", features = ["derive"] }
toml = "0.8"
thiserror = "2.0"
```

- [ ] **Step 2: Write the failing tests**

Create `src/config.rs` containing only the test module for now:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_has_sixty_second_cooldown() {
        let c = Config::default();
        assert_eq!(c.cooldown_secs, 60);
        assert!(c.monitoring_enabled);
        assert!(c.devices.is_empty());
        assert!(c.monitors.is_empty());
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let mut c = Config::default();
        c.cooldown_secs = 90;
        c.devices.push(DeviceEntry {
            id: "VID_046D&PID_085C".into(),
            name: "Logitech G502 HERO".into(),
            class: "Mouse".into(),
        });
        c.monitors.push(MonitorRule {
            serial: "ABC123456".into(),
            label: "Dell U2720Q".into(),
            on_connect: 15,
            on_disconnect: 17,
        });
        c.save(dir.path()).unwrap();

        let loaded = Config::load(dir.path()).unwrap();
        assert_eq!(loaded.cooldown_secs, 90);
        assert_eq!(loaded.devices[0].name, "Logitech G502 HERO");
        assert_eq!(loaded.monitors[0].on_connect, 15);
    }

    #[test]
    fn missing_file_yields_default_and_writes_it() {
        let dir = tempfile::tempdir().unwrap();
        let c = Config::load(dir.path()).unwrap();
        assert_eq!(c.cooldown_secs, 60);
        assert!(dir.path().join("config.toml").exists());
    }

    #[test]
    fn unparseable_file_is_quarantined_and_defaults_load() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.toml"), "this is not = = toml").unwrap();

        let c = Config::load(dir.path()).unwrap();
        assert_eq!(c.cooldown_secs, 60);
        assert!(dir.path().join("config.toml.broken").exists());
    }
}
```

Add the dev-dependency in `Cargo.toml`:

```toml
[dev-dependencies]
tempfile = "3"
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test --lib config 2>&1 | head -30`
Expected: compile errors — `cannot find type Config in this scope`.

- [ ] **Step 4: Write the implementation**

Prepend to `src/config.rs`, above the test module:

```rust
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const CONFIG_FILE: &str = "config.toml";
const CURRENT_VERSION: u32 = 2;

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("could not read or write {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("could not serialise config: {0}")]
    Serialise(#[from] toml::ser::Error),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeviceEntry {
    /// Always "VID_XXXX&PID_YYYY", uppercase hex.
    pub id: String,
    /// Cached friendly name so absent devices still display something useful.
    pub name: String,
    /// SetupAPI device class, used to pick the list icon.
    pub class: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MonitorRule {
    /// Stable key. Display indices shuffle across reboots; serials do not.
    pub serial: String,
    pub label: String,
    /// VCP 60 value to set when a watched device is present.
    pub on_connect: u16,
    /// VCP 60 value to set when no watched device is present.
    pub on_disconnect: u16,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Config {
    pub version: u32,
    pub cooldown_secs: u64,
    pub monitoring_enabled: bool,
    /// Empty means "look next to the executable".
    pub control_my_monitor_path: String,
    #[serde(default, rename = "devices")]
    pub devices: Vec<DeviceEntry>,
    #[serde(default, rename = "monitors")]
    pub monitors: Vec<MonitorRule>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            version: CURRENT_VERSION,
            cooldown_secs: 60,
            monitoring_enabled: true,
            control_my_monitor_path: String::new(),
            devices: Vec::new(),
            monitors: Vec::new(),
        }
    }
}

impl Config {
    /// Loads `config.toml` from `dir`.
    ///
    /// A missing file yields defaults, which are written to disk immediately.
    /// An unparseable file is renamed to `config.toml.broken` and defaults are
    /// used, so a corrupt config can never stop the app from starting.
    pub fn load(dir: &Path) -> Result<Config, ConfigError> {
        let path = dir.join(CONFIG_FILE);

        let raw = match std::fs::read_to_string(&path) {
            Ok(raw) => raw,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let c = Config::default();
                c.save(dir)?;
                return Ok(c);
            }
            Err(source) => return Err(ConfigError::Io { path, source }),
        };

        match toml::from_str::<Config>(&raw) {
            Ok(c) => Ok(c),
            Err(_) => {
                let broken = dir.join(format!("{CONFIG_FILE}.broken"));
                let _ = std::fs::rename(&path, &broken);
                let c = Config::default();
                c.save(dir)?;
                Ok(c)
            }
        }
    }

    pub fn save(&self, dir: &Path) -> Result<(), ConfigError> {
        let path = dir.join(CONFIG_FILE);
        let body = toml::to_string_pretty(self)?;
        std::fs::write(&path, body).map_err(|source| ConfigError::Io { path, source })
    }

    /// True when `id` (a "VID_XXXX&PID_YYYY" string) is watched.
    pub fn watches(&self, id: &str) -> bool {
        self.devices.iter().any(|d| d.id == id)
    }
}
```

Add `pub mod config;` to `src/lib.rs`.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test --lib config`
Expected: 4 tests pass.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml Cargo.lock src/config.rs src/main.rs
git commit -m "feat: add v2 config schema with load, save and corruption recovery"
```

---

## Task 2: Hardware types and the `/stext` parser

The parser is pure and cross-platform, so it is written and tested before any Win32 code.

**Files:**
- Create: `src/hardware/mod.rs`, `src/hardware/parse.rs`

**Interfaces:**
- Consumes: nothing
- Produces: `UsbDevice`, `MonitorInfo`, `DeviceClass`, `parse::parse_vid_pid(&str) -> Option<(u16, u16)>`, `parse::format_vid_pid(u16, u16) -> String`, `parse::extract_id_from_instance(&str) -> Option<String>`, `parse::parse_stext(&str) -> Vec<StextMonitor>`, `StextMonitor`

- [ ] **Step 1: Write the failing tests**

Create `src/hardware/parse.rs` with only the test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_vid_pid_pairs() {
        assert_eq!(parse_vid_pid("VID_046D&PID_085C"), Some((0x046D, 0x085C)));
        assert_eq!(parse_vid_pid("PID_085C&VID_046D"), Some((0x046D, 0x085C)));
        assert_eq!(parse_vid_pid("garbage"), None);
        assert_eq!(parse_vid_pid("VID_ZZZZ&PID_085C"), None);
    }

    #[test]
    fn formats_ids_as_uppercase_padded_hex() {
        assert_eq!(format_vid_pid(0x046D, 0x085C), "VID_046D&PID_085C");
        assert_eq!(format_vid_pid(0x1, 0x2), "VID_0001&PID_0002");
    }

    #[test]
    fn extracts_id_from_a_device_instance_path() {
        // Real SetupAPI instance IDs carry a trailing instance segment.
        assert_eq!(
            extract_id_from_instance(r"USB\VID_046D&PID_085C&MI_01\7&2A1B3C&0&0001"),
            Some("VID_046D&PID_085C".to_string())
        );
        assert_eq!(extract_id_from_instance(r"ACPI\PNP0C0C\2&daba3ff&2"), None);
    }

    #[test]
    fn extraction_is_case_insensitive_but_output_is_uppercase() {
        assert_eq!(
            extract_id_from_instance(r"usb\vid_046d&pid_085c\5&12ab"),
            Some("VID_046D&PID_085C".to_string())
        );
    }

    const SAMPLE_STEXT: &str = "\
==================================================
Monitor Device Name: \\\\.\\DISPLAY1\\Monitor0
Monitor Name        : DELL U2720Q
Serial Number       : ABC123456
VCP Code            : 60
VCP Code Name       : Input Select
Current Value       : 15
Maximum Value       : 18
==================================================
Monitor Device Name: \\\\.\\DISPLAY2\\Monitor0
Monitor Name        : LG HDR 4K
Serial Number       : XYZ987654
VCP Code            : 60
VCP Code Name       : Input Select
Current Value       : 17
Maximum Value       : 18
==================================================
";

    #[test]
    fn parses_stext_into_monitors() {
        let monitors = parse_stext(SAMPLE_STEXT);
        assert_eq!(monitors.len(), 2);
        assert_eq!(monitors[0].device_name, r"\\.\DISPLAY1\Monitor0");
        assert_eq!(monitors[0].model, "DELL U2720Q");
        assert_eq!(monitors[0].serial, "ABC123456");
        assert_eq!(monitors[0].current_input, Some(15));
        assert_eq!(monitors[1].serial, "XYZ987654");
        assert_eq!(monitors[1].current_input, Some(17));
    }

    #[test]
    fn ignores_vcp_codes_other_than_sixty() {
        let text = "\
Monitor Device Name: \\\\.\\DISPLAY1\\Monitor0
Monitor Name        : DELL U2720Q
Serial Number       : ABC123456
VCP Code            : 16
VCP Code Name       : Brightness
Current Value       : 75
Maximum Value       : 100
";
        let monitors = parse_stext(text);
        assert_eq!(monitors.len(), 1);
        assert_eq!(monitors[0].current_input, None);
    }

    #[test]
    fn tolerates_empty_or_garbage_input() {
        assert!(parse_stext("").is_empty());
        assert!(parse_stext("no colons here at all").is_empty());
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --lib parse 2>&1 | head -20`
Expected: compile errors — `cannot find function parse_vid_pid`.

- [ ] **Step 3: Write the parser**

Prepend to `src/hardware/parse.rs`:

```rust
/// One monitor block from a ControlMyMonitor `/stext` dump.
#[derive(Debug, Clone, PartialEq)]
pub struct StextMonitor {
    /// e.g. `\\.\DISPLAY1\Monitor0` — used only to correlate with GDI output.
    pub device_name: String,
    pub model: String,
    pub serial: String,
    /// Current VCP 60 value, if the dump contained a VCP 60 block.
    pub current_input: Option<u16>,
}

/// Parses "VID_046D&PID_085C" (either field order) into numeric ids.
pub fn parse_vid_pid(id: &str) -> Option<(u16, u16)> {
    let upper = id.to_ascii_uppercase();
    let mut vid = None;
    let mut pid = None;

    for part in upper.split(['&', '\\']) {
        if let Some(hex) = part.strip_prefix("VID_") {
            vid = u16::from_str_radix(hex, 16).ok();
        } else if let Some(hex) = part.strip_prefix("PID_") {
            pid = u16::from_str_radix(hex, 16).ok();
        }
    }

    Some((vid?, pid?))
}

/// Canonical form: uppercase hex, zero-padded to four digits.
pub fn format_vid_pid(vid: u16, pid: u16) -> String {
    format!("VID_{vid:04X}&PID_{pid:04X}")
}

/// Pulls the canonical VID/PID pair out of a SetupAPI device instance id.
///
/// Instance ids look like `USB\VID_046D&PID_085C&MI_01\7&2A1B3C&0&0001`; the
/// trailing `&MI_xx` interface segment and the instance suffix are dropped, so
/// every node belonging to one physical device collapses to the same string.
pub fn extract_id_from_instance(instance_id: &str) -> Option<String> {
    let (vid, pid) = parse_vid_pid(instance_id)?;
    Some(format_vid_pid(vid, pid))
}

/// Parses a ControlMyMonitor `/stext` dump.
///
/// The dump repeats a block per monitor per VCP code. Blocks are accumulated
/// by device name, and only the VCP 60 block contributes `current_input`.
// ponytail: line-oriented "Key: Value" scan rather than a grammar; the format
// is stable and NirSoft-generated. Revisit only if /stext output changes shape.
pub fn parse_stext(text: &str) -> Vec<StextMonitor> {
    let mut out: Vec<StextMonitor> = Vec::new();
    let mut device_name = String::new();
    let mut model = String::new();
    let mut serial = String::new();
    let mut vcp_code: Option<u16> = None;
    let mut current: Option<u16> = None;

    let flush = |out: &mut Vec<StextMonitor>,
                 device_name: &str,
                 model: &str,
                 serial: &str,
                 vcp_code: Option<u16>,
                 current: Option<u16>| {
        if device_name.is_empty() {
            return;
        }
        let input = if vcp_code == Some(60) { current } else { None };

        if let Some(existing) = out.iter_mut().find(|m: &&mut StextMonitor| m.device_name == device_name) {
            if input.is_some() {
                existing.current_input = input;
            }
            return;
        }

        out.push(StextMonitor {
            device_name: device_name.to_string(),
            model: model.to_string(),
            serial: serial.to_string(),
            current_input: input,
        });
    };

    for line in text.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim();

        match key {
            "Monitor Device Name" => {
                flush(&mut out, &device_name, &model, &serial, vcp_code, current);
                device_name = value.to_string();
                model.clear();
                serial.clear();
                vcp_code = None;
                current = None;
            }
            "Monitor Name" => model = value.to_string(),
            "Serial Number" => serial = value.to_string(),
            "VCP Code" => vcp_code = value.parse().ok(),
            "Current Value" => current = value.parse().ok(),
            _ => {}
        }
    }
    flush(&mut out, &device_name, &model, &serial, vcp_code, current);

    out
}
```

Note: `Monitor Device Name` values in the dump contain a colon only in the drive-letter sense, never in the key position, so `split_once(':')` is safe. `\\.\DISPLAY1\Monitor0` has no colon.

- [ ] **Step 4: Write the shared hardware types**

Create `src/hardware/mod.rs`:

```rust
pub mod parse;

#[cfg(windows)]
mod mon;
#[cfg(windows)]
mod usb;

#[cfg(not(windows))]
mod mock;

#[cfg(windows)]
pub use mon::{apply_input, list_monitors, read_input};
#[cfg(windows)]
pub use usb::list_devices;

#[cfg(not(windows))]
pub use mock::{apply_input, list_devices, list_monitors, read_input};

/// Broad category used to pick the icon in the device picker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceClass {
    Mouse,
    Keyboard,
    Camera,
    Hid,
    Other,
}

impl DeviceClass {
    /// Maps a SetupAPI `SPDRP_CLASS` string onto a category.
    pub fn from_setup_class(class: &str) -> Self {
        match class.to_ascii_lowercase().as_str() {
            "mouse" => Self::Mouse,
            "keyboard" => Self::Keyboard,
            "camera" | "image" | "media" => Self::Camera,
            "hidclass" => Self::Hid,
            _ => Self::Other,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Mouse => "Mouse",
            Self::Keyboard => "Keyboard",
            Self::Camera => "Camera",
            Self::Hid => "HIDClass",
            Self::Other => "Other",
        }
    }
}

/// A physically present USB device, deduplicated by VID&PID.
#[derive(Debug, Clone, PartialEq)]
pub struct UsbDevice {
    /// "VID_XXXX&PID_YYYY"
    pub id: String,
    pub name: String,
    pub class: DeviceClass,
}

/// A monitor, merging GDI geometry with ControlMyMonitor identity.
#[derive(Debug, Clone, PartialEq)]
pub struct MonitorInfo {
    pub serial: String,
    pub model: String,
    /// `\\.\DISPLAY1\Monitor0`, resolved fresh each enumeration.
    pub device_name: String,
    /// Virtual-desktop position, for the layout diagram.
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub is_primary: bool,
    pub current_input: Option<u16>,
}

#[derive(Debug, thiserror::Error)]
pub enum HardwareError {
    #[error("ControlMyMonitor.exe not found at {0}")]
    ToolMissing(String),
    #[error("ControlMyMonitor.exe failed: {0}")]
    ToolFailed(String),
    #[error("no monitor with serial {0}")]
    UnknownSerial(String),
    #[error("{0}")]
    Io(#[from] std::io::Error),
}
```

Add `pub mod hardware;` to `src/lib.rs`.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test --lib parse`
Expected: 7 tests pass. (`src/hardware/mod.rs` will not compile yet on either target because `usb`/`mon`/`mock` do not exist — create empty placeholder files with `// filled in by a later task` if needed to keep the build green, and remove the placeholders when those tasks land.)

- [ ] **Step 6: Commit**

```bash
git add src/hardware/
git commit -m "feat: add hardware types and ControlMyMonitor /stext parser"
```

---

## Task 3: v1 to v2 config migration

**Files:**
- Modify: `src/config.rs`

**Interfaces:**
- Consumes: `parse::parse_stext`, `StextMonitor`, `Config`, `DeviceEntry`, `MonitorRule`
- Produces: `MigrationError`, `Config::migrate_v1(raw: &str, monitors: &[StextMonitor], names: &dyn Fn(&str) -> Option<(String, String)>) -> Result<Config, MigrationError>`, `parse_set_value(&str) -> Option<(String, u16)>`

The serial resolver and the name resolver are passed in as data/closures rather than called directly, so migration is fully testable without Windows.

- [ ] **Step 1: Write the failing tests**

Append to the `tests` module in `src/config.rs`:

```rust
use crate::hardware::parse::StextMonitor;

fn sample_v1() -> &'static str {
    r#"
monitored_devices = [
    "VID_046D&PID_085C",
    "VID_3142&PID_0686",
]

disconnect_cmds = [
    '/SetValue "\\.\Display1\Monitor0" 60 17',
    '/SetValue "\\.\Display2\Monitor0" 60 15'
]

connect_cmds = [
    '/SetValue "\\.\Display1\Monitor0" 60 15',
    '/SetValue "\\.\Display2\Monitor0" 60 17'
]
"#
}

fn sample_monitors() -> Vec<StextMonitor> {
    vec![
        StextMonitor {
            device_name: r"\\.\DISPLAY1\Monitor0".into(),
            model: "DELL U2720Q".into(),
            serial: "ABC123456".into(),
            current_input: Some(15),
        },
        StextMonitor {
            device_name: r"\\.\DISPLAY2\Monitor0".into(),
            model: "LG HDR 4K".into(),
            serial: "XYZ987654".into(),
            current_input: Some(17),
        },
    ]
}

#[test]
fn parses_a_set_value_command() {
    assert_eq!(
        parse_set_value(r#"/SetValue "\\.\Display1\Monitor0" 60 15"#),
        Some((r"\\.\Display1\Monitor0".to_string(), 15))
    );
    assert_eq!(parse_set_value("/GetValue whatever 60"), None);
    assert_eq!(parse_set_value(""), None);
}

#[test]
fn migrates_devices_and_rules() {
    let names = |id: &str| match id {
        "VID_046D&PID_085C" => Some(("Logitech G502 HERO".to_string(), "Mouse".to_string())),
        _ => None,
    };

    let c = Config::migrate_v1(sample_v1(), &sample_monitors(), &names).unwrap();

    assert_eq!(c.version, 2);
    assert_eq!(c.cooldown_secs, 60);

    assert_eq!(c.devices.len(), 2);
    assert_eq!(c.devices[0].name, "Logitech G502 HERO");
    assert_eq!(c.devices[0].class, "Mouse");
    // An absent device keeps its id as its display name.
    assert_eq!(c.devices[1].name, "VID_3142&PID_0686");

    assert_eq!(c.monitors.len(), 2);
    let m1 = c.monitors.iter().find(|m| m.serial == "ABC123456").unwrap();
    assert_eq!(m1.on_connect, 15);
    assert_eq!(m1.on_disconnect, 17);
    let m2 = c.monitors.iter().find(|m| m.serial == "XYZ987654").unwrap();
    assert_eq!(m2.on_connect, 17);
    assert_eq!(m2.on_disconnect, 15);
}

#[test]
fn display_name_matching_ignores_case() {
    // v1 configs say "Display1", /stext says "DISPLAY1".
    let names = |_: &str| None;
    let c = Config::migrate_v1(sample_v1(), &sample_monitors(), &names).unwrap();
    assert_eq!(c.monitors.len(), 2);
}

#[test]
fn unresolvable_monitors_are_kept_as_unmatched() {
    let names = |_: &str| None;
    let only_one = vec![sample_monitors()[0].clone()];

    let c = Config::migrate_v1(sample_v1(), &only_one, &names).unwrap();

    assert_eq!(c.monitors.len(), 2);
    let unmatched = c.monitors.iter().find(|m| m.label == "Unmatched monitor").unwrap();
    assert!(unmatched.serial.is_empty());
    assert_eq!(unmatched.on_connect, 17);
    assert_eq!(unmatched.on_disconnect, 15);
}

#[test]
fn detects_a_v1_file() {
    assert!(is_v1(sample_v1()));
    assert!(!is_v1("version = 2\ncooldown_secs = 60\n"));
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --lib config 2>&1 | head -20`
Expected: `cannot find function parse_set_value` / `no function migrate_v1`.

- [ ] **Step 3: Write the migration**

Append to `src/config.rs` (above the tests module):

```rust
use crate::hardware::parse::StextMonitor;

#[derive(Debug, thiserror::Error)]
pub enum MigrationError {
    #[error("could not parse the v1 config: {0}")]
    Parse(#[from] toml::de::Error),
}

#[derive(Deserialize)]
struct ConfigV1 {
    #[serde(default)]
    monitored_devices: Vec<String>,
    #[serde(default)]
    connect_cmds: Vec<String>,
    #[serde(default)]
    disconnect_cmds: Vec<String>,
}

/// True when `raw` is a pre-versioning config.
pub fn is_v1(raw: &str) -> bool {
    !raw.lines().any(|l| l.trim_start().starts_with("version"))
}

/// Extracts the monitor argument and value from `/SetValue "<mon>" 60 <n>`.
pub fn parse_set_value(cmd: &str) -> Option<(String, u16)> {
    let rest = cmd.trim().strip_prefix("/SetValue")?.trim();

    let (monitor, tail) = if let Some(after_quote) = rest.strip_prefix('"') {
        let end = after_quote.find('"')?;
        (after_quote[..end].to_string(), &after_quote[end + 1..])
    } else {
        let end = rest.find(' ')?;
        (rest[..end].to_string(), &rest[end..])
    };

    let mut fields = tail.split_whitespace();
    let vcp: u16 = fields.next()?.parse().ok()?;
    if vcp != 60 {
        return None;
    }
    let value: u16 = fields.next()?.parse().ok()?;

    Some((monitor, value))
}

/// Normalises `\\.\Display1\Monitor0` for case-insensitive comparison.
fn norm(device_name: &str) -> String {
    device_name.to_ascii_uppercase()
}

impl Config {
    /// Converts a v1 config body into a v2 `Config`.
    ///
    /// `monitors` comes from a fresh `/stext` dump and supplies the display-name
    /// to serial mapping. `names` resolves a "VID_XXXX&PID_YYYY" to a
    /// (friendly name, class) pair for devices currently plugged in.
    ///
    /// Rules whose display name is not in `monitors` are retained with an empty
    /// serial and surfaced in the UI as unmatched, rather than being dropped.
    pub fn migrate_v1(
        raw: &str,
        monitors: &[StextMonitor],
        names: &dyn Fn(&str) -> Option<(String, String)>,
    ) -> Result<Config, MigrationError> {
        let old: ConfigV1 = toml::from_str(raw)?;
        let mut out = Config::default();

        out.devices = old
            .monitored_devices
            .iter()
            .map(|id| {
                let (name, class) = names(id)
                    .unwrap_or_else(|| (id.clone(), "Other".to_string()));
                DeviceEntry { id: id.clone(), name, class }
            })
            .collect();

        // Display name -> (serial, model)
        let by_display: Vec<(String, &StextMonitor)> =
            monitors.iter().map(|m| (norm(&m.device_name), m)).collect();

        // Display name -> (on_connect, on_disconnect), preserving first-seen order.
        let mut rules: Vec<(String, Option<u16>, Option<u16>)> = Vec::new();

        let mut record = |display: String, value: u16, is_connect: bool| {
            let key = norm(&display);
            match rules.iter_mut().find(|(d, _, _)| *d == key) {
                Some((_, on_c, on_d)) => {
                    if is_connect { *on_c = Some(value) } else { *on_d = Some(value) }
                }
                None => rules.push(if is_connect {
                    (key, Some(value), None)
                } else {
                    (key, None, Some(value))
                }),
            }
        };

        for cmd in &old.connect_cmds {
            if let Some((display, value)) = parse_set_value(cmd) {
                record(display, value, true);
            }
        }
        for cmd in &old.disconnect_cmds {
            if let Some((display, value)) = parse_set_value(cmd) {
                record(display, value, false);
            }
        }

        out.monitors = rules
            .into_iter()
            .map(|(display, on_connect, on_disconnect)| {
                match by_display.iter().find(|(d, _)| *d == display) {
                    Some((_, m)) => MonitorRule {
                        serial: m.serial.clone(),
                        label: m.model.clone(),
                        on_connect: on_connect.unwrap_or(0),
                        on_disconnect: on_disconnect.unwrap_or(0),
                    },
                    None => MonitorRule {
                        serial: String::new(),
                        label: "Unmatched monitor".to_string(),
                        on_connect: on_connect.unwrap_or(0),
                        on_disconnect: on_disconnect.unwrap_or(0),
                    },
                }
            })
            .collect();

        Ok(out)
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --lib config`
Expected: 9 tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/config.rs
git commit -m "feat: migrate v1 command-string configs to v2 rules"
```

---

## Task 4: Mock hardware backend

Written before the real backend so every later UI task is developable on Linux from the moment it lands.

**Files:**
- Create: `src/hardware/mock.rs`

**Interfaces:**
- Consumes: `UsbDevice`, `MonitorInfo`, `DeviceClass`, `HardwareError`
- Produces: `list_devices() -> Result<Vec<UsbDevice>, HardwareError>`, `list_monitors(tool: &Path) -> Result<Vec<MonitorInfo>, HardwareError>`, `read_input(tool: &Path, serial: &str) -> Result<u16, HardwareError>`, `apply_input(tool: &Path, serial: &str, value: u16) -> Result<(), HardwareError>`, `mock::set_device_present(id: &str, present: bool)`, `mock::set_tool_present(bool)`, `mock::set_fail_next_command(bool)`

- [ ] **Step 1: Write the failing test**

Create `src/hardware/mock.rs` with only:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn devices_are_deduplicated_by_vid_pid() {
        let devices = list_devices().unwrap();
        let mut ids: Vec<&str> = devices.iter().map(|d| d.id.as_str()).collect();
        ids.sort_unstable();
        let before = ids.len();
        ids.dedup();
        assert_eq!(before, ids.len(), "mock returned duplicate VID&PID entries");
    }

    #[test]
    fn unplugging_removes_a_device_from_the_list() {
        set_device_present("VID_046D&PID_085C", false);
        assert!(list_devices().unwrap().iter().all(|d| d.id != "VID_046D&PID_085C"));

        set_device_present("VID_046D&PID_085C", true);
        assert!(list_devices().unwrap().iter().any(|d| d.id == "VID_046D&PID_085C"));
    }

    #[test]
    fn applying_an_input_is_visible_to_the_next_read() {
        let tool = std::path::Path::new("ControlMyMonitor.exe");
        apply_input(tool, "ABC123456", 18).unwrap();
        assert_eq!(read_input(tool, "ABC123456").unwrap(), 18);
    }

    #[test]
    fn forced_failure_surfaces_as_tool_failed() {
        let tool = std::path::Path::new("ControlMyMonitor.exe");
        set_fail_next_command(true);
        let err = apply_input(tool, "ABC123456", 15).unwrap_err();
        assert!(matches!(err, HardwareError::ToolFailed(_)));
    }
}
```

Note: these tests mutate shared global state. Run them with `cargo test -- --test-threads=1` for this module, or accept that each test restores what it changed — the tests above are written to restore.

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --lib mock 2>&1 | head -20`
Expected: `cannot find function list_devices`.

- [ ] **Step 3: Write the mock**

Prepend to `src/hardware/mock.rs`:

```rust
//! Compile-time stand-in for the Win32 backend, used on non-Windows targets so
//! the UI can be developed and debugged on Linux. Never compiled on Windows.

use std::path::Path;
use std::sync::{Mutex, OnceLock};

use super::{DeviceClass, HardwareError, MonitorInfo, UsbDevice};

struct MockState {
    devices: Vec<(UsbDevice, bool)>,
    monitors: Vec<MonitorInfo>,
    tool_present: bool,
    fail_next: bool,
}

fn state() -> &'static Mutex<MockState> {
    static STATE: OnceLock<Mutex<MockState>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(MockState::new()))
}

impl MockState {
    fn new() -> Self {
        let dev = |id: &str, name: &str, class: DeviceClass| {
            (UsbDevice { id: id.into(), name: name.into(), class }, true)
        };

        // Eleven entries mirroring a real machine: a mix of properly named and
        // generic devices across several classes. The picker's deduplication
        // runs before this list is built, so entries here are already unique.
        let devices = vec![
            dev("VID_046D&PID_085C", "Logitech StreamCam", DeviceClass::Camera),
            dev("VID_046D&PID_C08B", "Logitech G502 HERO Gaming Mouse", DeviceClass::Mouse),
            dev("VID_3142&PID_0686", "USB Input Device", DeviceClass::Hid),
            dev("VID_1B1C&PID_1B7C", "Corsair K70 RGB Keyboard", DeviceClass::Keyboard),
            dev("VID_8087&PID_0032", "Intel Wireless Bluetooth", DeviceClass::Other),
            dev("VID_0BDA&PID_8153", "USB 10/100/1000 LAN", DeviceClass::Other),
            dev("VID_145F&PID_02A2", "USB Input Device", DeviceClass::Hid),
            dev("VID_05E3&PID_0610", "USB2.0 Hub", DeviceClass::Other),
            dev("VID_1462&PID_7C95", "USB Input Device", DeviceClass::Hid),
            dev("VID_0C45&PID_6366", "Trust Webcam", DeviceClass::Camera),
            dev("VID_04D9&PID_A052", "HID Keyboard Device", DeviceClass::Keyboard),
        ];

        // An L arrangement with a portrait secondary and a differently scaled
        // third screen, so the layout maths is genuinely exercised.
        let monitors = vec![
            MonitorInfo {
                serial: "ABC123456".into(),
                model: "DELL U2720Q".into(),
                device_name: r"\\.\DISPLAY1\Monitor0".into(),
                x: 0, y: 0, width: 3840, height: 2160,
                is_primary: true,
                current_input: Some(15),
            },
            MonitorInfo {
                serial: "XYZ987654".into(),
                model: "LG HDR 4K".into(),
                device_name: r"\\.\DISPLAY2\Monitor0".into(),
                x: 3840, y: -400, width: 1080, height: 1920,
                is_primary: false,
                current_input: Some(17),
            },
            MonitorInfo {
                serial: "QWE555111".into(),
                model: "AOC 24G2".into(),
                device_name: r"\\.\DISPLAY3\Monitor0".into(),
                x: -1920, y: 300, width: 1920, height: 1080,
                is_primary: false,
                current_input: Some(17),
            },
        ];

        Self { devices, monitors, tool_present: true, fail_next: false }
    }
}

pub fn list_devices() -> Result<Vec<UsbDevice>, HardwareError> {
    let s = state().lock().expect("mock state poisoned");
    Ok(s.devices
        .iter()
        .filter(|(_, present)| *present)
        .map(|(d, _)| d.clone())
        .collect())
}

pub fn list_monitors(_tool: &Path) -> Result<Vec<MonitorInfo>, HardwareError> {
    let s = state().lock().expect("mock state poisoned");
    if !s.tool_present {
        return Err(HardwareError::ToolMissing("ControlMyMonitor.exe".into()));
    }
    Ok(s.monitors.clone())
}

pub fn read_input(_tool: &Path, serial: &str) -> Result<u16, HardwareError> {
    let s = state().lock().expect("mock state poisoned");
    s.monitors
        .iter()
        .find(|m| m.serial == serial)
        .and_then(|m| m.current_input)
        .ok_or_else(|| HardwareError::UnknownSerial(serial.to_string()))
}

pub fn apply_input(_tool: &Path, serial: &str, value: u16) -> Result<(), HardwareError> {
    let mut s = state().lock().expect("mock state poisoned");

    if s.fail_next {
        s.fail_next = false;
        return Err(HardwareError::ToolFailed(
            "simulated failure: the monitor did not acknowledge the VCP write".into(),
        ));
    }
    if !s.tool_present {
        return Err(HardwareError::ToolMissing("ControlMyMonitor.exe".into()));
    }

    let m = s
        .monitors
        .iter_mut()
        .find(|m| m.serial == serial)
        .ok_or_else(|| HardwareError::UnknownSerial(serial.to_string()))?;
    m.current_input = Some(value);
    Ok(())
}

// -- controls driven by the debug panel ------------------------------------

pub fn set_device_present(id: &str, present: bool) {
    let mut s = state().lock().expect("mock state poisoned");
    if let Some((_, flag)) = s.devices.iter_mut().find(|(d, _)| d.id == id) {
        *flag = present;
    }
}

/// Unplugs or replugs every device at once — the KVM-toggle simulation.
pub fn set_all_present(present: bool) {
    let mut s = state().lock().expect("mock state poisoned");
    for (_, flag) in s.devices.iter_mut() {
        *flag = present;
    }
}

pub fn set_tool_present(present: bool) {
    state().lock().expect("mock state poisoned").tool_present = present;
}

pub fn set_fail_next_command(fail: bool) {
    state().lock().expect("mock state poisoned").fail_next = fail;
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --lib mock -- --test-threads=1`
Expected: 4 tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/hardware/mock.rs
git commit -m "feat: add mock hardware backend for Linux development"
```

---

## Task 5: SetupAPI USB enumeration

**Files:**
- Create: `src/hardware/usb.rs`
- Modify: `Cargo.toml` (remove `hidapi`, add the `Win32_Devices_Properties` feature)

**Interfaces:**
- Consumes: `parse::extract_id_from_instance`, `UsbDevice`, `DeviceClass`, `HardwareError`
- Produces: `list_devices() -> Result<Vec<UsbDevice>, HardwareError>` — same signature as the mock

This task is Windows-only and cannot be run on the development machine. Verify with `cargo check` on Linux (it must compile out cleanly) and by running on the Windows box.

- [ ] **Step 1: Update Cargo.toml**

Remove the `hidapi = "2.6"` line. Ensure the `windows` dependency reads:

```toml
windows = { version = "0.52", features = [
    "Win32_Foundation",
    "Win32_UI_WindowsAndMessaging",
    "Win32_Devices_DeviceAndDriverInstallation",
    "Win32_Devices_Properties",
    "Win32_System_LibraryLoader",
    "Win32_Graphics_Gdi",
] }
```

- [ ] **Step 2: Write the implementation**

Create `src/hardware/usb.rs`:

```rust
//! USB enumeration via SetupAPI.
//!
//! SetupAPI is used rather than hidapi because hidapi cannot see non-HID
//! devices (webcams, hubs) and reports generic product strings. SetupAPI
//! exposes the same friendly names Device Manager shows.

use windows::core::PCWSTR;
use windows::Win32::Devices::DeviceAndDriverInstallation::{
    SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInfo, SetupDiGetClassDevsW,
    SetupDiGetDeviceInstanceIdW, SetupDiGetDeviceRegistryPropertyW, DIGCF_ALLCLASSES,
    DIGCF_PRESENT, SPDRP_CLASS, SPDRP_DEVICEDESC, SPDRP_FRIENDLYNAME, SP_DEVINFO_DATA,
};

use super::parse::extract_id_from_instance;
use super::{DeviceClass, HardwareError, UsbDevice};

/// Enumerates every present device, keeping those carrying a VID/PID pair.
///
/// One physical device produces several device nodes (composite interfaces,
/// child HID collections), so results are deduplicated by VID&PID and the
/// most specific name wins.
pub fn list_devices() -> Result<Vec<UsbDevice>, HardwareError> {
    let mut out: Vec<UsbDevice> = Vec::new();

    // SAFETY: SetupDiGetClassDevsW with a null class GUID and DIGCF_ALLCLASSES
    // returns a handle to all present devices. The handle is released by
    // SetupDiDestroyDeviceInfoList below on every exit path.
    let devinfo = unsafe {
        SetupDiGetClassDevsW(None, PCWSTR::null(), None, DIGCF_PRESENT | DIGCF_ALLCLASSES)
            .map_err(|e| HardwareError::ToolFailed(format!("SetupDiGetClassDevs failed: {e}")))?
    };

    let mut index = 0u32;
    loop {
        let mut data = SP_DEVINFO_DATA {
            cbSize: std::mem::size_of::<SP_DEVINFO_DATA>() as u32,
            ..Default::default()
        };

        // SAFETY: `data.cbSize` is set as the API requires; enumeration stops
        // when this returns an error (ERROR_NO_MORE_ITEMS).
        let ok = unsafe { SetupDiEnumDeviceInfo(devinfo, index, &mut data) };
        if ok.is_err() {
            break;
        }
        index += 1;

        let Some(instance_id) = instance_id(devinfo, &data) else {
            continue;
        };
        let Some(id) = extract_id_from_instance(&instance_id) else {
            continue;
        };

        let name = registry_string(devinfo, &data, SPDRP_FRIENDLYNAME)
            .or_else(|| registry_string(devinfo, &data, SPDRP_DEVICEDESC))
            .unwrap_or_else(|| id.clone());

        let class = registry_string(devinfo, &data, SPDRP_CLASS)
            .map(|c| DeviceClass::from_setup_class(&c))
            .unwrap_or(DeviceClass::Other);

        match out.iter_mut().find(|d| d.id == id) {
            Some(existing) => {
                // Prefer a real name over a placeholder, and a specific class
                // over Other, whichever node happens to be enumerated first.
                if existing.name == existing.id && name != id {
                    existing.name = name;
                }
                if existing.class == DeviceClass::Other && class != DeviceClass::Other {
                    existing.class = class;
                }
            }
            None => out.push(UsbDevice { id, name, class }),
        }
    }

    // SAFETY: `devinfo` is a valid handle returned above and not used again.
    unsafe {
        let _ = SetupDiDestroyDeviceInfoList(devinfo);
    }

    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    Ok(out)
}

fn instance_id(
    devinfo: windows::Win32::Devices::DeviceAndDriverInstallation::HDEVINFO,
    data: &SP_DEVINFO_DATA,
) -> Option<String> {
    let mut buf = [0u16; 512];
    let mut needed = 0u32;

    // SAFETY: the buffer length is passed as its true element count; the call
    // writes at most that many UTF-16 code units.
    let ok = unsafe {
        SetupDiGetDeviceInstanceIdW(devinfo, data, Some(&mut buf), Some(&mut needed))
    };
    if ok.is_err() {
        return None;
    }
    Some(from_wide(&buf))
}

fn registry_string(
    devinfo: windows::Win32::Devices::DeviceAndDriverInstallation::HDEVINFO,
    data: &SP_DEVINFO_DATA,
    property: u32,
) -> Option<String> {
    let mut buf = [0u8; 1024];
    let mut needed = 0u32;

    // SAFETY: the byte buffer length is passed accurately. REG_SZ properties
    // are returned as UTF-16, which is decoded below.
    let ok = unsafe {
        SetupDiGetDeviceRegistryPropertyW(
            devinfo,
            data,
            property,
            None,
            Some(&mut buf),
            Some(&mut needed),
        )
    };
    if ok.is_err() {
        return None;
    }

    let wide: Vec<u16> = buf
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();

    let s = from_wide(&wide);
    if s.is_empty() { None } else { Some(s) }
}

fn from_wide(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}
```

- [ ] **Step 3: Verify it compiles out on Linux**

Run: `cargo check`
Expected: success, with `usb.rs` not compiled at all.

- [ ] **Step 4: Verify on Windows**

On the Windows machine, run `cargo check` and confirm success. If the `windows` 0.52 signatures differ from those above (buffer parameters are the usual difference), adapt the call sites — the enumeration structure stays as written.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock src/hardware/usb.rs
git commit -m "feat: enumerate USB devices via SetupAPI, replacing hidapi"
```

---

## Task 6: Windows monitor backend

**Files:**
- Create: `src/hardware/mon.rs`

**Interfaces:**
- Consumes: `parse::parse_stext`, `StextMonitor`, `MonitorInfo`, `HardwareError`
- Produces: `list_monitors(tool: &Path) -> Result<Vec<MonitorInfo>, HardwareError>`, `read_input(tool: &Path, serial: &str) -> Result<u16, HardwareError>`, `apply_input(tool: &Path, serial: &str, value: u16) -> Result<(), HardwareError>` — identical signatures to the mock

- [ ] **Step 1: Write the implementation**

Create `src/hardware/mon.rs`:

```rust
//! Monitor discovery and control.
//!
//! Geometry comes from GDI (`EnumDisplayMonitors`); identity and current input
//! come from a ControlMyMonitor `/stext` dump. The two are correlated on the
//! `\\.\DISPLAYn` prefix, which both sides report.

use std::path::Path;
use std::process::Command;

use windows::Win32::Foundation::{BOOL, LPARAM, RECT};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFOEXW, MONITORINFOF_PRIMARY,
};

use super::parse::parse_stext;
use super::{HardwareError, MonitorInfo};

struct GdiMonitor {
    /// `\\.\DISPLAY1`
    device: String,
    rect: RECT,
    is_primary: bool,
}

pub fn list_monitors(tool: &Path) -> Result<Vec<MonitorInfo>, HardwareError> {
    let gdi = enumerate_gdi();
    let stext = dump_stext(tool)?;

    let mut out = Vec::new();
    for s in stext {
        // `\\.\DISPLAY1\Monitor0` -> `\\.\DISPLAY1`
        let prefix = s
            .device_name
            .rsplit_once('\\')
            .map(|(head, _)| head.to_string())
            .unwrap_or_else(|| s.device_name.clone());

        let g = gdi
            .iter()
            .find(|g| g.device.eq_ignore_ascii_case(&prefix));

        out.push(MonitorInfo {
            serial: s.serial,
            model: s.model,
            device_name: s.device_name,
            x: g.map_or(0, |g| g.rect.left),
            y: g.map_or(0, |g| g.rect.top),
            width: g.map_or(1920, |g| g.rect.right - g.rect.left),
            height: g.map_or(1080, |g| g.rect.bottom - g.rect.top),
            is_primary: g.is_some_and(|g| g.is_primary),
            current_input: s.current_input,
        });
    }

    Ok(out)
}

pub fn read_input(tool: &Path, serial: &str) -> Result<u16, HardwareError> {
    dump_stext(tool)?
        .into_iter()
        .find(|m| m.serial == serial)
        .and_then(|m| m.current_input)
        .ok_or_else(|| HardwareError::UnknownSerial(serial.to_string()))
}

/// Sets VCP 60 on the monitor with the given serial.
///
/// ControlMyMonitor accepts a serial number directly as its monitor argument,
/// so no display-index lookup is needed at switch time.
pub fn apply_input(tool: &Path, serial: &str, value: u16) -> Result<(), HardwareError> {
    if !tool.exists() {
        return Err(HardwareError::ToolMissing(tool.display().to_string()));
    }

    let output = Command::new(tool)
        .args(["/SetValue", serial, "60", &value.to_string()])
        .output()?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let detail = if stderr.is_empty() {
            format!("exit code {:?}", output.status.code())
        } else {
            stderr
        };
        Err(HardwareError::ToolFailed(detail))
    }
}

/// Runs `/stext` into a temp file, parses it, and removes the file.
fn dump_stext(tool: &Path) -> Result<Vec<super::parse::StextMonitor>, HardwareError> {
    if !tool.exists() {
        return Err(HardwareError::ToolMissing(tool.display().to_string()));
    }

    let tmp = std::env::temp_dir().join(format!("monsw_{}.txt", std::process::id()));

    let output = Command::new(tool)
        .arg("/stext")
        .arg(&tmp)
        .output()?;

    if !output.status.success() {
        let _ = std::fs::remove_file(&tmp);
        return Err(HardwareError::ToolFailed(
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ));
    }

    // ControlMyMonitor writes UTF-16 when the /stext output is Unicode; reading
    // lossily covers both cases without sniffing the encoding.
    let bytes = std::fs::read(&tmp)?;
    let _ = std::fs::remove_file(&tmp);

    let text = decode(&bytes);
    Ok(parse_stext(&text))
}

fn decode(bytes: &[u8]) -> String {
    if bytes.starts_with(&[0xFF, 0xFE]) {
        let wide: Vec<u16> = bytes[2..]
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        String::from_utf16_lossy(&wide)
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    }
}

fn enumerate_gdi() -> Vec<GdiMonitor> {
    let mut result: Vec<GdiMonitor> = Vec::new();

    // SAFETY: the callback receives a pointer to `result` as its LPARAM and is
    // only invoked for the duration of this call, during which `result` is live.
    unsafe {
        let _ = EnumDisplayMonitors(
            None,
            None,
            Some(enum_proc),
            LPARAM(&mut result as *mut Vec<GdiMonitor> as isize),
        );
    }

    result
}

unsafe extern "system" fn enum_proc(
    hmonitor: HMONITOR,
    _hdc: HDC,
    _rect: *mut RECT,
    lparam: LPARAM,
) -> BOOL {
    let mut info = MONITORINFOEXW {
        monitorInfo: windows::Win32::Graphics::Gdi::MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFOEXW>() as u32,
            ..Default::default()
        },
        ..Default::default()
    };

    // SAFETY: cbSize is set to the extended struct size as GetMonitorInfoW
    // requires when passed a MONITORINFOEXW.
    let ok = unsafe {
        GetMonitorInfoW(hmonitor, &mut info as *mut MONITORINFOEXW as *mut _)
    };
    if !ok.as_bool() {
        return BOOL(1);
    }

    let end = info
        .szDevice
        .iter()
        .position(|&c| c == 0)
        .unwrap_or(info.szDevice.len());
    let device = String::from_utf16_lossy(&info.szDevice[..end]);

    // SAFETY: lparam carries the &mut Vec passed by enumerate_gdi, which
    // outlives this callback.
    let out = unsafe { &mut *(lparam.0 as *mut Vec<GdiMonitor>) };
    out.push(GdiMonitor {
        device,
        rect: info.monitorInfo.rcMonitor,
        is_primary: info.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0,
    });

    BOOL(1)
}
```

- [ ] **Step 2: Verify it compiles out on Linux**

Run: `cargo check && cargo test`
Expected: success; `mon.rs` is not compiled.

- [ ] **Step 3: Verify on Windows**

On the Windows machine: `cargo check`, then a one-off `cargo test -- --ignored` style manual check is not needed — this gets exercised for real in Task 13. Confirm compilation and that `list_monitors` returns your three monitors when called from a scratch `main`.

- [ ] **Step 4: Commit**

```bash
git add src/hardware/mon.rs
git commit -m "feat: add monitor enumeration and DDC control via ControlMyMonitor"
```

---

## Task 7: Watcher state machine

The switching logic, extracted as a pure function so it is fully testable on Linux. This is where the flapping bugs live.

**Files:**
- Create: `src/watcher.rs`

**Interfaces:**
- Consumes: nothing (deliberately — it takes `bool` and `Instant`, not hardware)
- Produces: `WatcherState`, `Action`, `WatcherState::new(cooldown: Duration)`, `WatcherState::evaluate(&mut self, present: bool, now: Instant) -> Option<Action>`, `WatcherState::tick(&mut self, now: Instant) -> Option<Action>`, `WatcherState::cooldown_remaining(&self, now: Instant) -> Option<Duration>`

- [ ] **Step 1: Write the failing tests**

Create `src/watcher.rs` with only the test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn state() -> (WatcherState, Instant) {
        (WatcherState::new(Duration::from_secs(60)), Instant::now())
    }

    #[test]
    fn first_evaluation_establishes_state_without_switching() {
        let (mut s, t0) = state();
        assert_eq!(s.evaluate(true, t0), None, "initial sync must not switch");
    }

    #[test]
    fn a_transition_produces_the_matching_action() {
        let (mut s, t0) = state();
        s.evaluate(true, t0);

        assert_eq!(s.evaluate(false, t0 + Duration::from_secs(1)), Some(Action::Disconnect));
    }

    #[test]
    fn no_action_when_state_is_unchanged() {
        let (mut s, t0) = state();
        s.evaluate(true, t0);
        assert_eq!(s.evaluate(true, t0 + Duration::from_secs(1)), None);
    }

    #[test]
    fn a_switch_starts_a_cooldown_that_suppresses_further_switches() {
        let (mut s, t0) = state();
        s.evaluate(true, t0);
        assert_eq!(s.evaluate(false, t0 + Duration::from_secs(1)), Some(Action::Disconnect));

        // Flip back inside the cooldown: suppressed.
        assert_eq!(s.evaluate(true, t0 + Duration::from_secs(5)), None);
        assert_eq!(s.evaluate(false, t0 + Duration::from_secs(10)), None);
    }

    #[test]
    fn a_flip_during_cooldown_is_honoured_at_expiry() {
        let (mut s, t0) = state();
        s.evaluate(true, t0);
        s.evaluate(false, t0 + Duration::from_secs(1));      // switch, cooldown starts
        s.evaluate(true, t0 + Duration::from_secs(5));       // suppressed, resync owed

        // At expiry the real state is re-applied rather than left wrong.
        assert_eq!(s.tick(t0 + Duration::from_secs(62)), Some(Action::Resync));
    }

    #[test]
    fn no_resync_when_nothing_happened_during_cooldown() {
        let (mut s, t0) = state();
        s.evaluate(true, t0);
        s.evaluate(false, t0 + Duration::from_secs(1));

        assert_eq!(s.tick(t0 + Duration::from_secs(62)), None);
    }

    #[test]
    fn tick_before_expiry_does_nothing() {
        let (mut s, t0) = state();
        s.evaluate(true, t0);
        s.evaluate(false, t0 + Duration::from_secs(1));
        s.evaluate(true, t0 + Duration::from_secs(5));

        assert_eq!(s.tick(t0 + Duration::from_secs(30)), None);
    }

    #[test]
    fn cooldown_remaining_counts_down_then_clears() {
        let (mut s, t0) = state();
        s.evaluate(true, t0);
        s.evaluate(false, t0 + Duration::from_secs(1));

        let left = s.cooldown_remaining(t0 + Duration::from_secs(11)).unwrap();
        assert_eq!(left.as_secs(), 50);
        assert!(s.cooldown_remaining(t0 + Duration::from_secs(120)).is_none());
    }

    #[test]
    fn after_cooldown_expires_switching_resumes_normally() {
        let (mut s, t0) = state();
        s.evaluate(true, t0);
        s.evaluate(false, t0 + Duration::from_secs(1));

        assert_eq!(s.evaluate(true, t0 + Duration::from_secs(70)), Some(Action::Connect));
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --lib watcher 2>&1 | head -20`
Expected: `cannot find type WatcherState`.

- [ ] **Step 3: Write the state machine**

Prepend to `src/watcher.rs`:

```rust
use std::time::{Duration, Instant};

/// What the caller should do to the monitors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Apply every rule's `on_connect` value.
    Connect,
    /// Apply every rule's `on_disconnect` value.
    Disconnect,
    /// Re-apply whichever set matches the current presence.
    ///
    /// Emitted when the device state changed during a cooldown; without this
    /// the monitors would be left showing the wrong input indefinitely.
    Resync,
}

/// Presence tracking with debounce-independent cooldown handling.
///
/// Deliberately free of I/O: it takes a `bool` and an `Instant`, so every
/// transition, suppression and resync is testable on any platform.
#[derive(Debug)]
pub struct WatcherState {
    cooldown: Duration,
    /// `None` until the first evaluation, which only establishes a baseline.
    last_state: Option<bool>,
    cooldown_until: Option<Instant>,
    /// The presence value the monitors were last actually switched to. A resync
    /// is owed only when this disagrees with `last_state` — a flip that returns
    /// to the already-applied value must NOT trigger one.
    applied: Option<bool>,
}

impl WatcherState {
    pub fn new(cooldown: Duration) -> Self {
        Self { cooldown, last_state: None, cooldown_until: None, applied: None }
    }

    /// Changing the cooldown does not resize one already in flight:
    /// `cooldown_until` is an absolute instant computed when the switch fired.
    /// The new value applies to the next cooldown.
    pub fn set_cooldown(&mut self, cooldown: Duration) {
        self.cooldown = cooldown;
    }

    pub fn last_state(&self) -> Option<bool> {
        self.last_state
    }

    /// Feeds an observed presence reading in.
    ///
    /// The very first call only records the baseline: the app must not switch
    /// monitors merely because it started up.
    pub fn evaluate(&mut self, present: bool, now: Instant) -> Option<Action> {
        let Some(previous) = self.last_state else {
            self.last_state = Some(present);
            self.applied = Some(present);
            return None;
        };

        if self.in_cooldown(now) {
            if present != previous {
                self.last_state = Some(present);
            }
            return None;
        }

        if present == previous {
            return None;
        }

        self.last_state = Some(present);
        self.applied = Some(present);
        self.cooldown_until = Some(self.arm(now));

        Some(if present { Action::Connect } else { Action::Disconnect })
    }

    /// Called periodically by the watcher thread to release a cooldown.
    ///
    /// This MUST be polled. A caller that only ever calls `evaluate` never
    /// delivers a flip that arrived mid-cooldown.
    pub fn tick(&mut self, now: Instant) -> Option<Action> {
        if self.in_cooldown(now) {
            return None;
        }

        let expired = self.cooldown_until.take().is_some();
        if expired && self.last_state != self.applied {
            self.applied = self.last_state;
            self.cooldown_until = Some(self.arm(now));
            return Some(Action::Resync);
        }
        None
    }

    pub fn cooldown_remaining(&self, now: Instant) -> Option<Duration> {
        self.cooldown_until
            .filter(|until| *until > now)
            .map(|until| until - now)
    }

    /// `cooldown_secs` is unvalidated in the config, so saturate rather than
    /// panic on `Instant` overflow.
    fn arm(&self, now: Instant) -> Instant {
        now.checked_add(self.cooldown).unwrap_or(now)
    }

    fn in_cooldown(&self, now: Instant) -> bool {
        self.cooldown_until.is_some_and(|until| now < until)
    }
}
```

Add `pub mod watcher;` to `src/lib.rs`.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --lib watcher`
Expected: 9 tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/watcher.rs src/main.rs
git commit -m "feat: add pure watcher state machine with cooldown and resync"
```

---

## Task 8: Watcher thread and headless wiring

Replaces the old `main.rs` logic. After this task the app behaves exactly as it does today — headless, tray-less for the moment — but on the new foundations.

**Files:**
- Modify: `src/watcher.rs`, `src/main.rs`
- Create: `src/app.rs`

**Interfaces:**
- Consumes: `Config`, `WatcherState`, `Action`, `hardware::list_devices`, `hardware::apply_input`
- Produces: `Event`, `LogEntry`, `Command`, `watcher::spawn(cfg: Arc<Mutex<Config>>, dir: PathBuf, tx: Sender<Event>) -> Sender<Command>`, `app::tool_path(cfg: &Config, dir: &Path) -> PathBuf`

- [ ] **Step 1: Define the channel messages**

Create `src/app.rs`:

```rust
use std::path::{Path, PathBuf};

use crate::config::Config;

/// Where ControlMyMonitor.exe lives: the configured override, else next to us.
pub fn tool_path(cfg: &Config, dir: &Path) -> PathBuf {
    if cfg.control_my_monitor_path.trim().is_empty() {
        dir.join("ControlMyMonitor.exe")
    } else {
        PathBuf::from(&cfg.control_my_monitor_path)
    }
}

/// The directory the app reads and writes its config in: alongside the exe.
pub fn app_dir() -> std::io::Result<PathBuf> {
    let exe = std::env::current_exe()?;
    exe.parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| std::io::Error::other("executable has no parent directory"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Info,
    Success,
    Warning,
    Error,
}

#[derive(Debug, Clone)]
pub struct LogEntry {
    /// Local wall-clock time, formatted for display.
    pub at: String,
    pub severity: Severity,
    pub message: String,
}

/// Watcher thread -> UI.
#[derive(Debug, Clone)]
pub enum Event {
    /// A watched device appeared or disappeared.
    PresenceChanged(bool),
    /// Cooldown seconds remaining, or None when idle.
    Cooldown(Option<u64>),
    Log(LogEntry),
}

/// UI -> watcher thread.
#[derive(Debug, Clone)]
pub enum Command {
    /// Configuration changed on disk; reload and re-read the cooldown.
    ConfigChanged,
    SetMonitoring(bool),
    /// Force a presence re-check (used by the Refresh button and the debug panel).
    Poke,
    Shutdown,
}
```

Add `pub mod app;` to `src/lib.rs`.

- [ ] **Step 2: Write the watcher thread**

Append to `src/watcher.rs`:

```rust
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};

use crate::app::{Command, Event, LogEntry, Severity};
use crate::config::Config;
use crate::hardware;

fn now_string() -> String {
    // ponytail: no chrono dependency for one timestamp. SystemTime -> HH:MM:SS
    // via seconds-of-day arithmetic; the date is never displayed.
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let sod = secs % 86_400;
    format!("{:02}:{:02}:{:02}", sod / 3600, (sod % 3600) / 60, sod % 60)
}

pub fn log(tx: &Sender<Event>, severity: Severity, message: impl Into<String>) {
    let _ = tx.send(Event::Log(LogEntry {
        at: now_string(),
        severity,
        message: message.into(),
    }));
}

/// Applies every configured rule for the given presence state.
fn apply_all(cfg: &Config, dir: &PathBuf, present: bool, tx: &Sender<Event>) {
    let tool = crate::app::tool_path(cfg, dir);

    for rule in &cfg.monitors {
        if rule.serial.is_empty() {
            log(tx, Severity::Warning,
                format!("Skipping \"{}\": no serial, re-detect it in Monitors", rule.label));
            continue;
        }

        let value = if present { rule.on_connect } else { rule.on_disconnect };
        match hardware::apply_input(&tool, &rule.serial, value) {
            Ok(()) => log(tx, Severity::Success,
                format!("{} -> input {}", rule.label, value)),
            Err(e) => log(tx, Severity::Error,
                format!("{} failed: {e}", rule.label)),
        }
    }
}

fn any_watched_present(cfg: &Config) -> bool {
    match hardware::list_devices() {
        Ok(devices) => devices.iter().any(|d| cfg.watches(&d.id)),
        Err(_) => false,
    }
}

/// Runs the watcher loop until a `Command::Shutdown` arrives.
///
/// `wake` is signalled by the platform layer whenever a device change is
/// observed; on Windows that is the hidden window's WM_DEVICECHANGE handler,
/// on other targets it is the debug panel.
pub fn run(
    cfg: Arc<Mutex<Config>>,
    dir: PathBuf,
    tx: Sender<Event>,
    commands: Receiver<Command>,
    wake: Receiver<()>,
) {
    let cooldown = {
        let c = cfg.lock().expect("config poisoned");
        Duration::from_secs(c.cooldown_secs)
    };
    let mut state = WatcherState::new(cooldown);
    let mut enabled = cfg.lock().expect("config poisoned").monitoring_enabled;

    // Establish the baseline without switching anything.
    {
        let c = cfg.lock().expect("config poisoned");
        let present = any_watched_present(&c);
        state.evaluate(present, Instant::now());
        let _ = tx.send(Event::PresenceChanged(present));
    }

    let mut last_cooldown_report: Option<u64> = None;

    loop {
        match commands.try_recv() {
            Ok(Command::Shutdown) | Err(TryRecvError::Disconnected) => return,
            Ok(Command::SetMonitoring(on)) => enabled = on,
            Ok(Command::ConfigChanged) => {
                let c = cfg.lock().expect("config poisoned");
                state.set_cooldown(Duration::from_secs(c.cooldown_secs));
                enabled = c.monitoring_enabled;
            }
            Ok(Command::Poke) | Err(TryRecvError::Empty) => {}
        }

        // Coalesce a burst of device-change notifications: a KVM toggle fires
        // several. Drain everything that arrived, then let it settle.
        let woken = wake.try_recv().is_ok();
        if woken {
            std::thread::sleep(Duration::from_millis(500));
            while wake.try_recv().is_ok() {}
        }

        let now = Instant::now();

        if enabled && woken {
            let c = cfg.lock().expect("config poisoned");
            let present = any_watched_present(&c);

            if state.last_state() != Some(present) {
                let _ = tx.send(Event::PresenceChanged(present));
            }

            if let Some(action) = state.evaluate(present, now) {
                let label = match action {
                    Action::Connect => "Watched device connected",
                    Action::Disconnect => "Watched device disconnected",
                    Action::Resync => "Resyncing after cooldown",
                };
                log(&tx, Severity::Info, label);
                apply_all(&c, &dir, present, &tx);
            }
        }

        if enabled && let Some(action) = state.tick(now) {
            debug_assert_eq!(action, Action::Resync);
            let c = cfg.lock().expect("config poisoned");
            let present = any_watched_present(&c);
            log(&tx, Severity::Info, "Cooldown expired, resyncing monitors");
            apply_all(&c, &dir, present, &tx);
        }

        let remaining = state.cooldown_remaining(now).map(|d| d.as_secs());
        if remaining != last_cooldown_report {
            let _ = tx.send(Event::Cooldown(remaining));
            last_cooldown_report = remaining;
        }

        std::thread::sleep(Duration::from_millis(200));
    }
}
```

- [ ] **Step 3: Write the platform wake source**

Append to `src/watcher.rs`:

```rust
/// Starts the platform's device-change notifier, returning the wake receiver.
///
/// On Windows this spawns the hidden-window message pump that has always
/// driven this application. On other targets there is no such thing, so the
/// sender is handed to the debug panel instead.
#[cfg(windows)]
pub fn spawn_wake_source() -> (Receiver<()>, Option<Sender<()>>) {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || win::pump(tx));
    (rx, None)
}

#[cfg(not(windows))]
pub fn spawn_wake_source() -> (Receiver<()>, Option<Sender<()>>) {
    let (tx, rx) = std::sync::mpsc::channel();
    (rx, Some(tx))
}

#[cfg(windows)]
mod win {
    use std::sync::mpsc::Sender;
    use std::sync::OnceLock;

    use windows::core::{w, PCWSTR};
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::Graphics::Gdi::HBRUSH;
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, PostQuitMessage,
        RegisterClassW, TranslateMessage, MSG, WINDOW_EX_STYLE, WM_DESTROY, WM_DEVICECHANGE,
        WNDCLASSW, WS_OVERLAPPED,
    };

    static WAKE: OnceLock<Sender<()>> = OnceLock::new();

    const DBT_DEVNODES_CHANGED: usize = 0x0007;
    const DBT_DEVICEARRIVAL: usize = 0x8000;
    const DBT_DEVICEREMOVECOMPLETE: usize = 0x8004;

    /// Creates a hidden message-only-ish window and pumps messages forever.
    ///
    /// A top-level window receives DBT_DEVNODES_CHANGED broadcasts without
    /// RegisterDeviceNotification, which is why no registration happens here.
    pub fn pump(tx: Sender<()>) {
        let _ = WAKE.set(tx);

        // SAFETY: GetModuleHandleW(None) returns this process's base address and
        // mutates nothing.
        let Ok(instance) = (unsafe { GetModuleHandleW(None) }) else {
            return;
        };

        let class_name = w!("MONITOR_SWITCHER_WATCHER");
        let wnd_class = WNDCLASSW {
            hInstance: instance.into(),
            lpszClassName: class_name,
            lpfnWndProc: Some(wnd_proc),
            hbrBackground: HBRUSH(0),
            ..Default::default()
        };

        // SAFETY: the class name and window procedure are defined in this
        // module and outlive the window.
        unsafe {
            if RegisterClassW(&raw const wnd_class) == 0 {
                return;
            }
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                class_name,
                PCWSTR::null(),
                WS_OVERLAPPED,
                0, 0, 0, 0,
                None, None, instance, None,
            );
            if hwnd.0 == 0 {
                return;
            }
        }

        let mut message = MSG::default();
        // SAFETY: standard Win32 message loop; GetMessageW blocks until a
        // message arrives and returns 0 on WM_QUIT.
        unsafe {
            loop {
                let result = GetMessageW(&raw mut message, None, 0, 0);
                if result.0 <= 0 {
                    return;
                }
                TranslateMessage(&raw const message);
                DispatchMessageW(&raw const message);
            }
        }
    }

    unsafe extern "system" fn wnd_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match msg {
            WM_DEVICECHANGE => {
                if matches!(
                    wparam.0,
                    DBT_DEVNODES_CHANGED | DBT_DEVICEARRIVAL | DBT_DEVICEREMOVECOMPLETE
                ) && let Some(tx) = WAKE.get()
                {
                    // Debounce and settle-delay live in the watcher loop, so
                    // this handler stays cheap and never blocks the pump.
                    let _ = tx.send(());
                }
                LRESULT(0)
            }
            WM_DESTROY => {
                // SAFETY: valid during window destruction.
                unsafe { PostQuitMessage(0) };
                LRESULT(0)
            }
            // SAFETY: the documented fallback for unhandled messages.
            _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
        }
    }
}
```

- [ ] **Step 4: Rewrite main.rs as a headless bootstrap**

Replace `src/main.rs` entirely:

```rust
use monitor_switcher::{app, config, hardware, watcher};

use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex};

use config::Config;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dir = app::app_dir()?;
    std::env::set_current_dir(&dir)?;

    let cfg = Arc::new(Mutex::new(Config::load(&dir)?));

    let (event_tx, event_rx) = channel();
    let (cmd_tx, cmd_rx) = channel();
    let (wake_rx, _debug_wake) = watcher::spawn_wake_source();

    {
        let cfg = Arc::clone(&cfg);
        let dir = dir.clone();
        std::thread::spawn(move || watcher::run(cfg, dir, event_tx, cmd_rx, wake_rx));
    }

    // Temporary headless console output; replaced by the UI in Task 9.
    for event in event_rx {
        println!("{event:?}");
    }

    drop(cmd_tx);
    Ok(())
}
```

- [ ] **Step 5: Verify**

Run: `cargo check && cargo test`
Expected: all tests pass on Linux; the binary compiles.

On Windows: `cargo run`, toggle the KVM, confirm the console prints presence changes and that monitors still switch.

- [ ] **Step 6: Commit**

```bash
git add src/app.rs src/main.rs src/watcher.rs
git commit -m "refactor: move switching logic into a watcher thread on the new config"
```

---

## Task 9: Dioxus shell, tray icon and stylesheet

**Files:**
- Modify: `Cargo.toml`, `src/main.rs`, `build.rs`
- Create: `src/ui/mod.rs`, `assets/style.css`

**Interfaces:**
- Consumes: `Event`, `Command`, `Config`, `LogEntry`
- Produces: `ui::launch(cfg, dir, event_rx, cmd_tx, debug_wake)`, `ui::AppState` (Dioxus context: `Signal<Config>`, `Signal<Vec<LogEntry>>`, `Signal<bool>` presence, `Signal<Option<u64>>` cooldown), `ui::Tab`

- [ ] **Step 1: Add the Dioxus dependency**

In `Cargo.toml`:

```toml
dioxus = { version = "0.7.10", features = ["desktop"] }
```

- [ ] **Step 2: Stop bundling a sample config**

`build.rs` currently copies `config.toml` next to the binary, which would overwrite a user's real config on every build and now also fights the generated default. Edit `build.rs` so `files_to_copy` is:

```rust
    let files_to_copy = ["icon.ico"];
```

and drop the `cargo:rerun-if-changed=config.toml` line.

- [ ] **Step 3: Write the stylesheet**

Create `assets/style.css`:

```css
:root {
  --bg: #1e1f22;
  --bg-panel: #2b2d30;
  --bg-raised: #393b40;
  --border: #43454a;
  --text: #dfe1e5;
  --text-dim: #9da0a8;
  --accent: #3574f0;
  --ok: #4c9f50;
  --warn: #d9a343;
  --err: #d64f4f;
  --radius: 6px;
}

* { box-sizing: border-box; }

body {
  margin: 0;
  font: 14px/1.5 "Segoe UI", system-ui, sans-serif;
  background: var(--bg);
  color: var(--text);
}

.shell { display: flex; height: 100vh; }

.sidebar {
  width: 190px;
  flex: 0 0 190px;
  background: var(--bg-panel);
  border-right: 1px solid var(--border);
  padding: 12px 8px;
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.sidebar button {
  background: none;
  border: 0;
  color: var(--text-dim);
  text-align: left;
  padding: 8px 12px;
  border-radius: var(--radius);
  cursor: pointer;
  font: inherit;
}
.sidebar button:hover { background: var(--bg-raised); color: var(--text); }
.sidebar button.active { background: var(--accent); color: #fff; }

.content { flex: 1; overflow-y: auto; padding: 20px 24px; }

h1 { font-size: 18px; font-weight: 600; margin: 0 0 16px; }
h2 { font-size: 14px; font-weight: 600; margin: 20px 0 8px; color: var(--text-dim); }

.card {
  background: var(--bg-panel);
  border: 1px solid var(--border);
  border-radius: var(--radius);
  padding: 16px;
  margin-bottom: 16px;
}

.pill {
  display: inline-flex;
  align-items: center;
  gap: 8px;
  padding: 10px 18px;
  border-radius: 999px;
  font-weight: 600;
  font-size: 15px;
}
.pill.on  { background: rgba(76,159,80,.18);  color: var(--ok);  }
.pill.off { background: rgba(214,79,79,.18);  color: var(--err); }

.banner {
  border-radius: var(--radius);
  padding: 10px 14px;
  margin-bottom: 14px;
  border: 1px solid transparent;
}
.banner.warn { background: rgba(217,163,67,.14); border-color: var(--warn); }
.banner.err  { background: rgba(214,79,79,.14);  border-color: var(--err);  }

table { width: 100%; border-collapse: collapse; }
th, td { text-align: left; padding: 8px 10px; border-bottom: 1px solid var(--border); }
th { color: var(--text-dim); font-weight: 500; font-size: 12px; text-transform: uppercase; }
tr.absent td { opacity: .5; }

button.primary {
  background: var(--accent); color: #fff; border: 0;
  padding: 7px 14px; border-radius: var(--radius); cursor: pointer; font: inherit;
}
button.secondary {
  background: var(--bg-raised); color: var(--text); border: 1px solid var(--border);
  padding: 7px 14px; border-radius: var(--radius); cursor: pointer; font: inherit;
}
button:disabled { opacity: .45; cursor: default; }

input[type="text"], input[type="number"], select {
  background: var(--bg); color: var(--text);
  border: 1px solid var(--border); border-radius: var(--radius);
  padding: 6px 8px; font: inherit;
}

.log {
  font-family: "Cascadia Mono", ui-monospace, monospace;
  font-size: 12.5px;
  max-height: 320px;
  overflow-y: auto;
}
.log div { padding: 2px 0; }
.log .info { color: var(--text-dim); }
.log .success { color: var(--ok); }
.log .warning { color: var(--warn); }
.log .error { color: var(--err); }

.layout {
  position: relative;
  background: var(--bg);
  border: 1px solid var(--border);
  border-radius: var(--radius);
  height: 280px;
  margin-bottom: 16px;
}
.screen {
  position: absolute;
  background: var(--bg-raised);
  border: 2px solid var(--border);
  border-radius: 4px;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  cursor: pointer;
  overflow: hidden;
}
.screen.selected { border-color: var(--accent); }
.screen .num { font-size: 26px; font-weight: 700; }
.screen .meta { font-size: 11px; color: var(--text-dim); }
```

- [ ] **Step 4: Write the app root**

Create `src/ui/mod.rs`:

```rust
mod dashboard;
mod devices;
mod monitors;
mod settings;

#[cfg(not(windows))]
mod debug;

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};

use dioxus::prelude::*;

use crate::app::{Command, Event, LogEntry};
use crate::config::Config;

const MAX_LOG: usize = 200;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Dashboard,
    Devices,
    Monitors,
    Settings,
    #[cfg(not(windows))]
    Debug,
}

/// Everything the views read and write, provided via context.
#[derive(Clone, Copy)]
pub struct AppState {
    pub config: Signal<Config>,
    pub log: Signal<Vec<LogEntry>>,
    pub present: Signal<bool>,
    pub cooldown: Signal<Option<u64>>,
    pub tool_ok: Signal<bool>,
}

/// Shared handles the views need for side effects.
#[derive(Clone)]
pub struct Handles {
    pub dir: PathBuf,
    pub shared_config: Arc<Mutex<Config>>,
    pub commands: Sender<Command>,
    #[cfg(not(windows))]
    pub wake: Option<Sender<()>>,
}

impl Handles {
    /// Persists the UI's config, mirrors it to the watcher, and notifies it.
    pub fn save(&self, cfg: &Config) {
        if let Err(e) = cfg.save(&self.dir) {
            eprintln!("could not save config: {e}");
            return;
        }
        if let Ok(mut shared) = self.shared_config.lock() {
            *shared = cfg.clone();
        }
        let _ = self.commands.send(Command::ConfigChanged);
    }
}

pub fn launch(
    initial: Config,
    handles: Handles,
    events: Receiver<Event>,
) {
    let props = RootProps { initial, handles, events: Arc::new(Mutex::new(Some(events))) };

    dioxus::LaunchBuilder::desktop()
        .with_cfg(
            dioxus::desktop::Config::new()
                .with_window(
                    dioxus::desktop::WindowBuilder::new()
                        .with_title("Monitor Switcher")
                        .with_inner_size(dioxus::desktop::LogicalSize::new(980.0, 680.0)),
                )
                .with_menu(None),
        )
        .with_context(props)
        .launch(Root);
}

#[derive(Clone)]
struct RootProps {
    initial: Config,
    handles: Handles,
    events: Arc<Mutex<Option<Receiver<Event>>>>,
}

#[component]
fn Root() -> Element {
    let props = use_context::<RootProps>();

    let state = AppState {
        config: use_signal(|| props.initial.clone()),
        log: use_signal(Vec::new),
        present: use_signal(|| false),
        cooldown: use_signal(|| None),
        tool_ok: use_signal(|| true),
    };
    use_context_provider(|| state);
    use_context_provider(|| props.handles.clone());

    let mut tab = use_signal(|| Tab::Dashboard);

    // Drain the watcher channel into signals. The receiver is blocking, so it
    // lives on its own thread and hands values back through a Dioxus coroutine.
    let _pump = use_coroutine({
        let events = Arc::clone(&props.events);
        move |_rx: UnboundedReceiver<()>| {
            let mut state = state;
            let events = Arc::clone(&events);
            async move {
                let Some(rx) = events.lock().ok().and_then(|mut g| g.take()) else {
                    return;
                };
                let (async_tx, mut async_rx) = futures_channel::mpsc::unbounded();
                std::thread::spawn(move || {
                    for ev in rx {
                        if async_tx.unbounded_send(ev).is_err() {
                            return;
                        }
                    }
                });

                use futures_util::StreamExt;
                while let Some(ev) = async_rx.next().await {
                    match ev {
                        Event::PresenceChanged(p) => state.present.set(p),
                        Event::Cooldown(c) => state.cooldown.set(c),
                        Event::Log(entry) => {
                            state.log.with_mut(|l| {
                                l.push(entry);
                                if l.len() > MAX_LOG {
                                    let overflow = l.len() - MAX_LOG;
                                    l.drain(0..overflow);
                                }
                            });
                        }
                    }
                }
            }
        }
    });

    rsx! {
        document::Style { {include_str!("../../assets/style.css")} }
        div { class: "shell",
            nav { class: "sidebar",
                NavButton { tab: Tab::Dashboard, current: tab, label: "Dashboard" }
                NavButton { tab: Tab::Devices,   current: tab, label: "Devices" }
                NavButton { tab: Tab::Monitors,  current: tab, label: "Monitors" }
                NavButton { tab: Tab::Settings,  current: tab, label: "Settings" }
                #[cfg(not(windows))]
                NavButton { tab: Tab::Debug, current: tab, label: "Debug" }
            }
            main { class: "content",
                match tab() {
                    Tab::Dashboard => rsx! { dashboard::Dashboard {} },
                    Tab::Devices   => rsx! { devices::Devices {} },
                    Tab::Monitors  => rsx! { monitors::Monitors {} },
                    Tab::Settings  => rsx! { settings::Settings {} },
                    #[cfg(not(windows))]
                    Tab::Debug     => rsx! { debug::Debug {} },
                }
            }
        }
    }
}

#[component]
fn NavButton(tab: Tab, current: Signal<Tab>, label: String) -> Element {
    let active = current() == tab;
    rsx! {
        button {
            class: if active { "active" } else { "" },
            onclick: move |_| current.set(tab),
            "{label}"
        }
    }
}
```

Add to `Cargo.toml`:

```toml
futures-channel = "0.3"
futures-util = "0.3"
```

- [ ] **Step 5: Wire the tray icon and hide-on-close in main.rs**

Replace the temporary event-printing loop in `src/main.rs`:

```rust
use monitor_switcher::{app, config, hardware, ui, watcher};

use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex};

use config::Config;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dir = app::app_dir()?;
    std::env::set_current_dir(&dir)?;

    let cfg = Config::load(&dir)?;
    let shared = Arc::new(Mutex::new(cfg.clone()));

    let (event_tx, event_rx) = channel();
    let (cmd_tx, cmd_rx) = channel();
    let (wake_rx, debug_wake) = watcher::spawn_wake_source();

    {
        let shared = Arc::clone(&shared);
        let dir = dir.clone();
        std::thread::spawn(move || watcher::run(shared, dir, event_tx, cmd_rx, wake_rx));
    }

    let handles = ui::Handles {
        dir,
        shared_config: Arc::clone(&shared),
        commands: cmd_tx,
        #[cfg(not(windows))]
        wake: debug_wake,
    };

    #[cfg(windows)]
    let _ = debug_wake;

    ui::launch(cfg, handles, event_rx);
    Ok(())
}
```

Tray icon: create it inside the Dioxus window's setup so it lives on the event-loop thread. In `ui::launch`, after building the config, add a `.with_custom_event_handler` or create the tray inside `Root`'s `use_effect` guarded by `#[cfg(windows)]`:

```rust
// in Root, after the signals are set up
#[cfg(windows)]
use_effect(move || {
    use tray_icon::{menu::{Menu, MenuItem}, TrayIconBuilder};

    let menu = Menu::new();
    let open = MenuItem::new("Open", true, None);
    let quit = MenuItem::new("Quit", true, None);
    let _ = menu.append(&open);
    let _ = menu.append(&quit);

    if let Ok(icon) = tray_icon::Icon::from_path("icon.ico", Some((32, 32)))
        && let Ok(tray) = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip("Monitor Switcher")
            .with_icon(icon)
            .build()
    {
        // Leaked deliberately: the tray must outlive this effect and lives for
        // the whole process.
        // ponytail: Box::leak instead of threading a handle through app state.
        Box::leak(Box::new(tray));
    }
});
```

Hide-on-close: use `dioxus::desktop::use_window()` and intercept the close request so it calls `window.set_visible(false)` instead of exiting. Consult the Dioxus 0.7 desktop docs for the exact hook name in this version; the behaviour required is "closing the window hides it, only the tray Quit item exits the process".

- [ ] **Step 6: Create placeholder views so it compiles**

Create `src/ui/dashboard.rs`, `devices.rs`, `monitors.rs`, `settings.rs`, and (non-windows) `debug.rs`, each with:

```rust
use dioxus::prelude::*;

#[component]
pub fn Dashboard() -> Element {   // rename per file
    rsx! { h1 { "Dashboard" } }
}
```

- [ ] **Step 7: Verify**

Run: `cargo run`
Expected on Linux: a window opens with a dark sidebar and five nav items including Debug; clicking switches the heading.

- [ ] **Step 8: Commit**

```bash
git add Cargo.toml Cargo.lock build.rs assets/ src/main.rs src/ui/
git commit -m "feat: add Dioxus shell, sidebar navigation, tray icon and dark theme"
```

---

## Task 10: Dashboard view

**Files:**
- Modify: `src/ui/dashboard.rs`

**Interfaces:**
- Consumes: `AppState`, `Handles`, `Command`, `Severity`
- Produces: the `Dashboard` component

- [ ] **Step 1: Write the view**

Replace `src/ui/dashboard.rs`:

```rust
use dioxus::prelude::*;

use crate::app::{Command, Severity};
use crate::ui::{AppState, Handles};

#[component]
pub fn Dashboard() -> Element {
    let state = use_context::<AppState>();
    let handles = use_context::<Handles>();

    let mut config = state.config;
    let present = (state.present)();
    let cooldown = (state.cooldown)();
    let enabled = config().monitoring_enabled;

    rsx! {
        h1 { "Dashboard" }

        if !(state.tool_ok)() {
            div { class: "banner err",
                "ControlMyMonitor.exe was not found. Monitoring is paused — set it up in Settings."
            }
        }

        div { class: "card",
            div { class: if present { "pill on" } else { "pill off" },
                if present { "KVM Connected" } else { "KVM Disconnected" }
            }

            div { style: "margin-top:14px; display:flex; align-items:center; gap:14px;",
                button {
                    class: if enabled { "secondary" } else { "primary" },
                    onclick: move |_| {
                        let mut c = config();
                        c.monitoring_enabled = !c.monitoring_enabled;
                        let _ = handles.commands.send(Command::SetMonitoring(c.monitoring_enabled));
                        handles.save(&c);
                        config.set(c);
                    },
                    if enabled { "Pause monitoring" } else { "Resume monitoring" }
                }

                if let Some(secs) = cooldown {
                    span { style: "color:var(--text-dim)",
                        "Cooldown: {secs}s remaining — switches are suppressed"
                    }
                }
            }
        }

        h2 { "Activity" }
        div { class: "card log",
            if (state.log)().is_empty() {
                div { class: "info", "Nothing yet. Toggle your KVM to see events here." }
            }
            for entry in (state.log)().iter().rev() {
                div {
                    class: match entry.severity {
                        Severity::Info => "info",
                        Severity::Success => "success",
                        Severity::Warning => "warning",
                        Severity::Error => "error",
                    },
                    "[{entry.at}] {entry.message}"
                }
            }
        }
    }
}
```

- [ ] **Step 2: Verify**

Run: `cargo run`, go to Debug (Task 11 adds the controls; until then the log stays empty). Confirm the pill renders and the pause button toggles and persists across restarts.

- [ ] **Step 3: Commit**

```bash
git add src/ui/dashboard.rs
git commit -m "feat: add dashboard with status pill, monitoring toggle and event log"
```

---

## Task 11: Debug panel

Landed before the remaining views so they can be exercised on Linux.

**Files:**
- Modify: `src/ui/debug.rs`

**Interfaces:**
- Consumes: `Handles.wake`, `hardware::mock`
- Produces: the `Debug` component (non-windows only)

- [ ] **Step 1: Export the mock controls**

In `src/hardware/mod.rs`, add below the existing re-exports:

```rust
#[cfg(not(windows))]
pub use mock::{set_all_present, set_device_present, set_fail_next_command, set_tool_present};
```

- [ ] **Step 2: Write the panel**

Replace `src/ui/debug.rs`:

```rust
//! Simulation controls for developing on Linux, where no WM_DEVICECHANGE
//! exists. Events are pushed through the same channel the Windows message
//! pump uses, so the code path under test is the real one.

use dioxus::prelude::*;

use crate::hardware;
use crate::ui::{AppState, Handles};

#[component]
pub fn Debug() -> Element {
    let state = use_context::<AppState>();
    let handles = use_context::<Handles>();

    let poke = {
        let handles = handles.clone();
        move || {
            if let Some(wake) = &handles.wake {
                let _ = wake.send(());
            }
        }
    };

    rsx! {
        h1 { "Debug" }
        div { class: "banner warn",
            "This panel exists only on non-Windows builds. It simulates the device
             notifications Windows would deliver."
        }

        div { class: "card",
            h2 { "KVM simulation" }
            div { style: "display:flex; gap:8px; flex-wrap:wrap;",
                button { class: "primary",
                    onclick: {
                        let poke = poke.clone();
                        move |_| { hardware::set_all_present(true); poke(); }
                    },
                    "Plug everything in"
                }
                button { class: "secondary",
                    onclick: {
                        let poke = poke.clone();
                        move |_| { hardware::set_all_present(false); poke(); }
                    },
                    "Unplug everything"
                }
                button { class: "secondary",
                    onclick: {
                        let poke = poke.clone();
                        move |_| {
                            // Rapid toggle: exercises the 500ms settle window
                            // and the cooldown suppression path.
                            for i in 0..6 {
                                hardware::set_all_present(i % 2 == 0);
                                poke();
                            }
                        }
                    },
                    "Rapid toggle x6"
                }
            }
        }

        div { class: "card",
            h2 { "Individual devices" }
            table {
                thead { tr { th { "Device" } th { "ID" } th { "" } } }
                tbody {
                    for d in hardware::list_devices().unwrap_or_default() {
                        tr {
                            td { "{d.name}" }
                            td { "{d.id}" }
                            td {
                                button { class: "secondary",
                                    onclick: {
                                        let poke = poke.clone();
                                        let id = d.id.clone();
                                        move |_| { hardware::set_device_present(&id, false); poke(); }
                                    },
                                    "Unplug"
                                }
                            }
                        }
                    }
                }
            }
        }

        div { class: "card",
            h2 { "Failure injection" }
            div { style: "display:flex; gap:8px; flex-wrap:wrap;",
                button { class: "secondary",
                    onclick: move |_| {
                        hardware::set_tool_present(false);
                        state.tool_ok.clone().set(false);
                    },
                    "Hide ControlMyMonitor.exe"
                }
                button { class: "secondary",
                    onclick: move |_| {
                        hardware::set_tool_present(true);
                        state.tool_ok.clone().set(true);
                    },
                    "Restore ControlMyMonitor.exe"
                }
                button { class: "secondary",
                    onclick: move |_| hardware::set_fail_next_command(true),
                    "Fail the next switch"
                }
                button { class: "secondary",
                    onclick: move |_| {
                        let mut c = (state.config)();
                        c.monitors.push(crate::config::MonitorRule {
                            serial: String::new(),
                            label: "Unmatched monitor".into(),
                            on_connect: 15,
                            on_disconnect: 17,
                        });
                        handles.save(&c);
                        state.config.clone().set(c);
                    },
                    "Add an unmatched monitor rule"
                }
            }
        }
    }
}
```

- [ ] **Step 3: Verify**

Run: `cargo run`. On the Debug tab press "Unplug everything", switch to Dashboard: the pill flips to KVM Disconnected and the log records the switch. Press "Rapid toggle x6": only one switch occurs, then the cooldown counter appears. Press "Fail the next switch" then toggle: a red log entry appears.

- [ ] **Step 4: Commit**

```bash
git add src/hardware/mod.rs src/ui/debug.rs
git commit -m "feat: add Linux debug panel simulating device change events"
```

---

## Task 12: Devices view

**Files:**
- Modify: `src/ui/devices.rs`

**Interfaces:**
- Consumes: `hardware::list_devices`, `UsbDevice`, `DeviceClass`, `Config`, `DeviceEntry`, `Handles::save`
- Produces: the `Devices` component

- [ ] **Step 1: Write the view**

Replace `src/ui/devices.rs`:

```rust
use dioxus::prelude::*;

use crate::config::DeviceEntry;
use crate::hardware::{self, DeviceClass};
use crate::ui::{AppState, Handles};

fn icon(class: DeviceClass) -> &'static str {
    match class {
        DeviceClass::Mouse => "🖱",
        DeviceClass::Keyboard => "⌨",
        DeviceClass::Camera => "📷",
        DeviceClass::Hid => "🎛",
        DeviceClass::Other => "🔌",
    }
}

#[component]
pub fn Devices() -> Element {
    let state = use_context::<AppState>();
    let handles = use_context::<Handles>();

    let mut config = state.config;
    let mut query = use_signal(String::new);
    let mut only_watched = use_signal(|| false);
    let mut refresh = use_signal(|| 0u32);

    // Re-enumerates whenever `refresh` changes.
    let present = use_memo(move || {
        let _ = refresh();
        hardware::list_devices().unwrap_or_default()
    });

    let cfg = config();
    let needle = query().to_lowercase();

    let visible: Vec<_> = present()
        .into_iter()
        .filter(|d| {
            (!only_watched() || cfg.watches(&d.id))
                && (needle.is_empty()
                    || d.name.to_lowercase().contains(&needle)
                    || d.id.to_lowercase().contains(&needle))
        })
        .collect();

    // Configured devices that are not currently plugged in.
    let absent: Vec<_> = cfg
        .devices
        .iter()
        .filter(|e| !present().iter().any(|d| d.id == e.id))
        .cloned()
        .collect();

    rsx! {
        h1 { "Devices" }
        p { style: "color:var(--text-dim); margin-top:-8px;",
            "Switch on the devices that move with your KVM. When any of them appears
             or disappears, your monitors follow."
        }

        div { style: "display:flex; gap:10px; align-items:center; margin-bottom:14px;",
            input {
                r#type: "text",
                placeholder: "Search devices",
                value: "{query}",
                style: "flex:1",
                oninput: move |e| query.set(e.value()),
            }
            label { style: "display:flex; gap:6px; align-items:center; color:var(--text-dim)",
                input {
                    r#type: "checkbox",
                    checked: only_watched(),
                    onchange: move |e| only_watched.set(e.checked()),
                }
                "Watched only"
            }
            button { class: "secondary", onclick: move |_| refresh += 1, "Refresh" }
        }

        div { class: "card",
            table {
                thead {
                    tr { th { "" } th { "Device" } th { "Hardware ID" } th { "Watch" } }
                }
                tbody {
                    for d in visible {
                        tr {
                            key: "{d.id}",
                            td { "{icon(d.class)}" }
                            td { "{d.name}" }
                            td { style: "color:var(--text-dim); font-family:monospace", "{d.id}" }
                            td {
                                input {
                                    r#type: "checkbox",
                                    checked: cfg.watches(&d.id),
                                    onchange: {
                                        let d = d.clone();
                                        let handles = handles.clone();
                                        move |e: Event<FormData>| {
                                            let mut c = config();
                                            if e.checked() {
                                                if !c.watches(&d.id) {
                                                    c.devices.push(DeviceEntry {
                                                        id: d.id.clone(),
                                                        name: d.name.clone(),
                                                        class: d.class.as_str().to_string(),
                                                    });
                                                }
                                            } else {
                                                c.devices.retain(|x| x.id != d.id);
                                            }
                                            handles.save(&c);
                                            config.set(c);
                                        }
                                    },
                                }
                            }
                        }
                    }
                }
            }
        }

        if !absent.is_empty() {
            h2 { "Watched but not currently connected" }
            div { class: "card",
                table {
                    tbody {
                        for e in absent {
                            tr { class: "absent", key: "{e.id}",
                                td { "{e.name}" }
                                td { style: "font-family:monospace", "{e.id}" }
                                td {
                                    button { class: "secondary",
                                        onclick: {
                                            let id = e.id.clone();
                                            let handles = handles.clone();
                                            move |_| {
                                                let mut c = config();
                                                c.devices.retain(|x| x.id != id);
                                                handles.save(&c);
                                                config.set(c);
                                            }
                                        },
                                        "Remove"
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
```

- [ ] **Step 2: Verify**

Run: `cargo run`. Eleven mock devices appear with icons; search filters; ticking one writes it to `config.toml`; unplugging it in the Debug panel then returning to Devices moves it into the greyed section with a working Remove button.

- [ ] **Step 3: Commit**

```bash
git add src/ui/devices.rs
git commit -m "feat: add device picker with search, watch toggles and absent devices"
```

---

## Task 13: Monitors view — layout and rules

**Files:**
- Modify: `src/ui/monitors.rs`

**Interfaces:**
- Consumes: `hardware::list_monitors`, `MonitorInfo`, `MonitorRule`, `app::tool_path`
- Produces: the `Monitors` component, `INPUT_PRESETS`, `scale_layout(&[MonitorInfo], f64, f64) -> Vec<(usize, f64, f64, f64, f64)>`

- [ ] **Step 1: Write the failing test for the layout maths**

Add to `src/ui/monitors.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::hardware::MonitorInfo;

    fn m(x: i32, y: i32, w: i32, h: i32) -> MonitorInfo {
        MonitorInfo {
            serial: format!("S{x}"),
            model: "Test".into(),
            device_name: "dev".into(),
            x, y, width: w, height: h,
            is_primary: false,
            current_input: None,
        }
    }

    #[test]
    fn layout_fits_inside_the_canvas_and_preserves_relative_position() {
        let monitors = vec![m(0, 0, 3840, 2160), m(3840, -400, 1080, 1920)];
        let boxes = scale_layout(&monitors, 800.0, 280.0);

        assert_eq!(boxes.len(), 2);
        for (_, left, top, w, h) in &boxes {
            assert!(*left >= 0.0 && *top >= 0.0);
            assert!(left + w <= 800.5, "box overflows the canvas width");
            assert!(top + h <= 280.5, "box overflows the canvas height");
        }
        // The second monitor sits to the right of and above the first.
        assert!(boxes[1].1 > boxes[0].1);
        assert!(boxes[1].2 < boxes[0].2);
    }

    #[test]
    fn aspect_ratio_is_preserved() {
        let monitors = vec![m(0, 0, 1920, 1080)];
        let (_, _, _, w, h) = scale_layout(&monitors, 800.0, 280.0)[0];
        assert!(((w / h) - (1920.0 / 1080.0)).abs() < 0.01);
    }

    #[test]
    fn an_empty_list_yields_no_boxes() {
        assert!(scale_layout(&[], 800.0, 280.0).is_empty());
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --lib monitors 2>&1 | head -20`
Expected: `cannot find function scale_layout`.

- [ ] **Step 3: Write the view**

Replace the top of `src/ui/monitors.rs` (keeping the test module at the bottom):

```rust
use dioxus::prelude::*;

use crate::app::tool_path;
use crate::config::MonitorRule;
use crate::hardware::{self, MonitorInfo};
use crate::ui::{AppState, Handles};

/// Standard DDC/CI input-select values. Vendors deviate, which is why the
/// custom field and the Test button exist.
pub const INPUT_PRESETS: &[(u16, &str)] = &[
    (1, "VGA"),
    (3, "DVI-1"),
    (4, "DVI-2"),
    (15, "DisplayPort 1"),
    (16, "DisplayPort 2"),
    (17, "HDMI 1"),
    (18, "HDMI 2"),
];

pub fn input_label(value: u16) -> String {
    INPUT_PRESETS
        .iter()
        .find(|(v, _)| *v == value)
        .map(|(v, name)| format!("{name} ({v})"))
        .unwrap_or_else(|| format!("Custom ({value})"))
}

/// Projects virtual-desktop rectangles onto a canvas of `cw` x `ch` pixels.
///
/// Returns `(index, left, top, width, height)` per monitor. A single uniform
/// scale is used so relative sizes and gaps survive the projection.
pub fn scale_layout(
    monitors: &[MonitorInfo],
    cw: f64,
    ch: f64,
) -> Vec<(usize, f64, f64, f64, f64)> {
    if monitors.is_empty() {
        return Vec::new();
    }

    let min_x = monitors.iter().map(|m| m.x).min().unwrap_or(0) as f64;
    let min_y = monitors.iter().map(|m| m.y).min().unwrap_or(0) as f64;
    let max_x = monitors.iter().map(|m| m.x + m.width).max().unwrap_or(1) as f64;
    let max_y = monitors.iter().map(|m| m.y + m.height).max().unwrap_or(1) as f64;

    let span_x = (max_x - min_x).max(1.0);
    let span_y = (max_y - min_y).max(1.0);

    const PAD: f64 = 16.0;
    let scale = ((cw - PAD * 2.0) / span_x).min((ch - PAD * 2.0) / span_y);

    // Centre the whole arrangement in the canvas.
    let off_x = (cw - span_x * scale) / 2.0;
    let off_y = (ch - span_y * scale) / 2.0;

    monitors
        .iter()
        .enumerate()
        .map(|(i, m)| {
            (
                i,
                (m.x as f64 - min_x) * scale + off_x,
                (m.y as f64 - min_y) * scale + off_y,
                m.width as f64 * scale,
                m.height as f64 * scale,
            )
        })
        .collect()
}

#[component]
pub fn Monitors() -> Element {
    let state = use_context::<AppState>();
    let handles = use_context::<Handles>();

    let mut config = state.config;
    let mut refresh = use_signal(|| 0u32);
    let mut selected = use_signal(|| 0usize);

    let detected = use_memo(move || {
        let _ = refresh();
        let cfg = config();
        let tool = tool_path(&cfg, &handles.dir);
        hardware::list_monitors(&tool).unwrap_or_default()
    });

    let monitors = detected();
    let boxes = scale_layout(&monitors, 800.0, 280.0);

    rsx! {
        h1 { "Monitors" }
        p { style: "color:var(--text-dim); margin-top:-8px;",
            "Click a screen to choose which input it should show when the KVM is
             connected and when it is not."
        }

        div { style: "display:flex; gap:8px; margin-bottom:12px;",
            button { class: "secondary", onclick: move |_| refresh += 1, "Refresh" }
        }

        div { class: "layout",
            if monitors.is_empty() {
                div { style: "padding:20px; color:var(--text-dim)",
                    "No monitors detected. Check ControlMyMonitor.exe in Settings."
                }
            }
            for (i, left, top, w, h) in boxes {
                div {
                    key: "{monitors[i].serial}",
                    class: if selected() == i { "screen selected" } else { "screen" },
                    style: "left:{left}px; top:{top}px; width:{w}px; height:{h}px;",
                    onclick: move |_| selected.set(i),
                    div { class: "num", "{i + 1}" }
                    div { class: "meta", "{monitors[i].model}" }
                    div { class: "meta",
                        {monitors[i].current_input.map(input_label).unwrap_or_else(|| "input ?".into())}
                    }
                }
            }
        }

        if let Some(mon) = monitors.get(selected()) {
            MonitorPanel { monitor: mon.clone() }
        }

        {
            let cfg = config();
            let unmatched: Vec<MonitorRule> = cfg
                .monitors
                .iter()
                .filter(|r| r.serial.is_empty() || !monitors.iter().any(|m| m.serial == r.serial))
                .cloned()
                .collect();

            rsx! {
                if !unmatched.is_empty() {
                    h2 { "Unmatched rules" }
                    div { class: "card",
                        p { style: "color:var(--text-dim)",
                            "These rules reference monitors that are not currently detected.
                             They are kept so nothing is lost, but they will be skipped."
                        }
                        table {
                            tbody {
                                for r in unmatched {
                                    tr { key: "{r.label}-{r.on_connect}",
                                        td { "{r.label}" }
                                        td { "connected → {input_label(r.on_connect)}" }
                                        td { "disconnected → {input_label(r.on_disconnect)}" }
                                        td {
                                            button { class: "secondary",
                                                onclick: {
                                                    let r = r.clone();
                                                    let handles = handles.clone();
                                                    move |_| {
                                                        let mut c = config();
                                                        c.monitors.retain(|x| {
                                                            !(x.serial == r.serial && x.label == r.label)
                                                        });
                                                        handles.save(&c);
                                                        config.set(c);
                                                    }
                                                },
                                                "Remove"
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --lib monitors`
Expected: 3 tests pass. (`MonitorPanel` does not exist yet — add a temporary stub component returning `rsx! { div {} }` so this compiles; Task 14 fills it in.)

- [ ] **Step 5: Commit**

```bash
git add src/ui/monitors.rs
git commit -m "feat: add to-scale monitor layout with selection and unmatched rules"
```

---

## Task 14: Monitor input rules and the test flow

**Files:**
- Modify: `src/ui/monitors.rs`

**Interfaces:**
- Consumes: `MonitorInfo`, `MonitorRule`, `hardware::read_input`, `hardware::apply_input`, `INPUT_PRESETS`
- Produces: `MonitorPanel` component

The test flow reverts **before** asking, because during the test the GUI may be displayed on an input the user cannot see.

- [ ] **Step 1: Write the panel**

Replace the `MonitorPanel` stub in `src/ui/monitors.rs`:

```rust
#[derive(Clone, PartialEq)]
enum TestPhase {
    Idle,
    /// Showing the candidate, counting down before the automatic revert.
    Running { candidate: u16, seconds_left: u8 },
    /// Reverted; waiting for the user to say what to do with the candidate.
    Asking { candidate: u16 },
    Failed { message: String },
}

const TEST_SECONDS: u8 = 8;

#[component]
fn MonitorPanel(monitor: MonitorInfo) -> Element {
    let state = use_context::<AppState>();
    let handles = use_context::<Handles>();

    let mut config = state.config;
    let mut phase = use_signal(|| TestPhase::Idle);
    let mut candidate = use_signal(|| 15u16);

    let cfg = config();
    let rule = cfg
        .monitors
        .iter()
        .find(|r| r.serial == monitor.serial)
        .cloned()
        .unwrap_or(MonitorRule {
            serial: monitor.serial.clone(),
            label: monitor.model.clone(),
            on_connect: monitor.current_input.unwrap_or(15),
            on_disconnect: monitor.current_input.unwrap_or(17),
        });

    // Writes a rule field, creating the rule if this monitor has none yet.
    let write_rule = {
        let handles = handles.clone();
        let serial = monitor.serial.clone();
        let label = monitor.model.clone();
        move |on_connect: Option<u16>, on_disconnect: Option<u16>| {
            let mut c = config();
            match c.monitors.iter_mut().find(|r| r.serial == serial) {
                Some(existing) => {
                    if let Some(v) = on_connect { existing.on_connect = v; }
                    if let Some(v) = on_disconnect { existing.on_disconnect = v; }
                    existing.label = label.clone();
                }
                None => c.monitors.push(MonitorRule {
                    serial: serial.clone(),
                    label: label.clone(),
                    on_connect: on_connect.unwrap_or(15),
                    on_disconnect: on_disconnect.unwrap_or(17),
                }),
            }
            handles.save(&c);
            config.set(c);
        }
    };

    let cooling_down = (state.cooldown)().is_some();
    let testing = !matches!(phase(), TestPhase::Idle);

    rsx! {
        div { class: "card",
            h2 { "{monitor.model}" }
            p { style: "color:var(--text-dim); margin-top:-4px; font-family:monospace; font-size:12px;",
                "serial {monitor.serial} · {monitor.width}×{monitor.height}"
                if monitor.is_primary { " · primary" }
            }

            div { style: "display:flex; gap:24px; flex-wrap:wrap; margin-top:12px;",
                InputChooser {
                    label: "When the KVM is connected",
                    value: rule.on_connect,
                    on_change: {
                        let write_rule = write_rule.clone();
                        move |v| write_rule(Some(v), None)
                    },
                }
                InputChooser {
                    label: "When the KVM is disconnected",
                    value: rule.on_disconnect,
                    on_change: {
                        let write_rule = write_rule.clone();
                        move |v| write_rule(None, Some(v))
                    },
                }
            }
        }

        div { class: "card",
            h2 { "Test an input" }
            p { style: "color:var(--text-dim); margin-top:-4px;",
                "The monitor switches to the chosen input for {TEST_SECONDS} seconds, then
                 switches back on its own. You are asked what to do with it afterwards, so
                 you are never stranded on an input you cannot see."
            }

            match phase() {
                TestPhase::Idle => rsx! {
                    div { style: "display:flex; gap:10px; align-items:center;",
                        select {
                            value: "{candidate}",
                            onchange: move |e| {
                                if let Ok(v) = e.value().parse::<u16>() { candidate.set(v) }
                            },
                            for (v, name) in INPUT_PRESETS {
                                option { value: "{v}", "{name} ({v})" }
                            }
                        }
                        input {
                            r#type: "number", min: "0", max: "255",
                            style: "width:90px",
                            value: "{candidate}",
                            oninput: move |e| {
                                if let Ok(v) = e.value().parse::<u16>() { candidate.set(v) }
                            },
                        }
                        button {
                            class: "primary",
                            disabled: cooling_down,
                            onclick: {
                                let handles = handles.clone();
                                let serial = monitor.serial.clone();
                                move |_| {
                                    let cfg = config();
                                    let tool = tool_path(&cfg, &handles.dir);
                                    let target = candidate();

                                    let previous = match hardware::read_input(&tool, &serial) {
                                        Ok(v) => v,
                                        Err(e) => {
                                            phase.set(TestPhase::Failed { message: e.to_string() });
                                            return;
                                        }
                                    };
                                    if let Err(e) = hardware::apply_input(&tool, &serial, target) {
                                        phase.set(TestPhase::Failed { message: e.to_string() });
                                        return;
                                    }

                                    phase.set(TestPhase::Running {
                                        candidate: target,
                                        seconds_left: TEST_SECONDS,
                                    });

                                    // Count down, then revert unconditionally.
                                    let serial = serial.clone();
                                    let tool = tool.clone();
                                    spawn(async move {
                                        for remaining in (0..TEST_SECONDS).rev() {
                                            gloo_timers::future::TimeoutFuture::new(1_000).await;
                                            phase.set(TestPhase::Running {
                                                candidate: target,
                                                seconds_left: remaining,
                                            });
                                        }
                                        match hardware::apply_input(&tool, &serial, previous) {
                                            Ok(()) => phase.set(TestPhase::Asking { candidate: target }),
                                            Err(e) => phase.set(TestPhase::Failed {
                                                message: format!("could not revert: {e}"),
                                            }),
                                        }
                                    });
                                }
                            },
                            "Test"
                        }
                        if cooling_down {
                            span { style: "color:var(--text-dim)",
                                "Testing is paused while a switch cooldown is active."
                            }
                        }
                    }
                },

                TestPhase::Running { candidate, seconds_left } => rsx! {
                    div { class: "banner warn",
                        "Showing {input_label(candidate)} — reverting in {seconds_left}s."
                    }
                },

                TestPhase::Asking { candidate } => rsx! {
                    div { class: "card", style: "background:var(--bg-raised)",
                        p { "Did {input_label(candidate)} show the right source?" }
                        div { style: "display:flex; gap:8px; flex-wrap:wrap;",
                            button { class: "primary",
                                onclick: {
                                    let write_rule = write_rule.clone();
                                    move |_| { write_rule(Some(candidate), None); phase.set(TestPhase::Idle); }
                                },
                                "Use when KVM connected"
                            }
                            button { class: "primary",
                                onclick: {
                                    let write_rule = write_rule.clone();
                                    move |_| { write_rule(None, Some(candidate)); phase.set(TestPhase::Idle); }
                                },
                                "Use when KVM disconnected"
                            }
                            button { class: "secondary",
                                onclick: {
                                    let write_rule = write_rule.clone();
                                    move |_| {
                                        write_rule(Some(candidate), Some(candidate));
                                        phase.set(TestPhase::Idle);
                                    }
                                },
                                "Use for both"
                            }
                            button { class: "secondary",
                                onclick: move |_| phase.set(TestPhase::Idle),
                                "Discard"
                            }
                        }
                    }
                },

                TestPhase::Failed { message } => rsx! {
                    div { class: "banner err", "Test failed: {message}" }
                    button { class: "secondary", onclick: move |_| phase.set(TestPhase::Idle), "Back" }
                },
            }
        }
    }
}

#[component]
fn InputChooser(label: String, value: u16, on_change: EventHandler<u16>) -> Element {
    rsx! {
        div {
            div { style: "color:var(--text-dim); margin-bottom:6px;", "{label}" }
            select {
                value: "{value}",
                onchange: move |e| {
                    if let Ok(v) = e.value().parse::<u16>() { on_change.call(v) }
                },
                for (v, name) in INPUT_PRESETS {
                    option { value: "{v}", "{name} ({v})" }
                }
                if !INPUT_PRESETS.iter().any(|(v, _)| *v == value) {
                    option { value: "{value}", selected: true, "Custom ({value})" }
                }
            }
            input {
                r#type: "number", min: "0", max: "255",
                style: "width:90px; margin-left:8px;",
                value: "{value}",
                oninput: move |e| {
                    if let Ok(v) = e.value().parse::<u16>() { on_change.call(v) }
                },
            }
        }
    }
}
```

Add to `Cargo.toml` for the countdown timer:

```toml
gloo-timers = { version = "0.3", features = ["futures"] }
```

If `gloo-timers` does not work in the desktop (non-wasm) context in Dioxus 0.7, replace the `TimeoutFuture` calls with a thread-based countdown that sends progress over a channel, or with Dioxus's own `use_future` + `async_std`/`smol` timer — the requirement is one tick per second for `TEST_SECONDS`, then a revert.

- [ ] **Step 2: Verify**

Run: `cargo run`, open Monitors, select a screen, press Test. The banner counts 8 → 0, the mock's `current_input` visibly changes and reverts, and the four-button dialog appears. Choosing "Use when KVM connected" writes `on_connect` to `config.toml`.

On Windows, repeat the test against a real monitor and confirm the physical input switches and reverts.

- [ ] **Step 3: Commit**

```bash
git add Cargo.toml Cargo.lock src/ui/monitors.rs
git commit -m "feat: add input rules and revert-first input testing"
```

---

## Task 15: Settings, dependency assistant and startup shortcut

**Files:**
- Create: `src/deps.rs`, `src/startup.rs`
- Modify: `src/ui/settings.rs`, `Cargo.toml`, `src/main.rs`

**Interfaces:**
- Consumes: `Config`, `app::tool_path`
- Produces: `deps::status(&Path) -> ToolStatus`, `deps::download_to(&Path) -> Result<(), DepsError>`, `startup::is_enabled() -> bool`, `startup::set_enabled(bool) -> Result<(), StartupError>`

- [ ] **Step 1: Write the dependency helper**

Create `src/deps.rs`:

```rust
use std::io::Read;
use std::path::Path;

const DOWNLOAD_URL: &str = "https://www.nirsoft.net/utils/controlmymonitor.zip";
const EXE_NAME: &str = "ControlMyMonitor.exe";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolStatus {
    Found,
    Missing,
}

#[derive(Debug, thiserror::Error)]
pub enum DepsError {
    #[error("download failed: {0}")]
    Download(String),
    #[error("the archive did not contain {EXE_NAME}")]
    NotInArchive,
    #[error("{0}")]
    Io(#[from] std::io::Error),
}

pub fn status(tool: &Path) -> ToolStatus {
    if tool.is_file() { ToolStatus::Found } else { ToolStatus::Missing }
}

/// Downloads NirSoft's zip and extracts ControlMyMonitor.exe into `dir`.
///
/// ControlMyMonitor is NirSoft's work, redistributed here only by fetching it
/// from their own site at the user's request.
pub fn download_to(dir: &Path) -> Result<(), DepsError> {
    let mut body = Vec::new();
    ureq::get(DOWNLOAD_URL)
        .call()
        .map_err(|e| DepsError::Download(e.to_string()))?
        .into_body()
        .into_reader()
        .read_to_end(&mut body)?;

    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(body))
        .map_err(|e| DepsError::Download(e.to_string()))?;

    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| DepsError::Download(e.to_string()))?;

        let is_target = entry
            .enclosed_name()
            .and_then(|p| p.file_name().map(|f| f.eq_ignore_ascii_case(EXE_NAME)))
            .unwrap_or(false);

        if is_target {
            let mut out = std::fs::File::create(dir.join(EXE_NAME))?;
            std::io::copy(&mut entry, &mut out)?;
            return Ok(());
        }
    }

    Err(DepsError::NotInArchive)
}
```

Add to `Cargo.toml`:

```toml
ureq = "3.3"
zip = { version = "8.6", default-features = false, features = ["deflate"] }
```

- [ ] **Step 2: Write the startup helper**

Create `src/startup.rs`:

```rust
//! "Run at startup" via a shortcut in the user's Startup folder.

#[derive(Debug, thiserror::Error)]
pub enum StartupError {
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Other(String),
}

#[cfg(windows)]
mod imp {
    use std::path::PathBuf;

    use super::StartupError;

    const LINK_NAME: &str = "Monitor Switcher.lnk";

    fn startup_dir() -> Result<PathBuf, StartupError> {
        let appdata = std::env::var("APPDATA")
            .map_err(|_| StartupError::Other("APPDATA is not set".into()))?;
        Ok(PathBuf::from(appdata)
            .join(r"Microsoft\Windows\Start Menu\Programs\Startup"))
    }

    pub fn is_enabled() -> bool {
        startup_dir().map(|d| d.join(LINK_NAME).exists()).unwrap_or(false)
    }

    /// Creates or removes the shortcut.
    ///
    // ponytail: shells out to PowerShell's WScript.Shell rather than pulling in
    // a COM/IShellLink binding for one .lnk. Swap to IShellLink only if the
    // PowerShell dependency ever becomes a problem.
    pub fn set_enabled(enabled: bool) -> Result<(), StartupError> {
        let link = startup_dir()?.join(LINK_NAME);

        if !enabled {
            if link.exists() {
                std::fs::remove_file(&link)?;
            }
            return Ok(());
        }

        let exe = std::env::current_exe()?;
        let dir = exe
            .parent()
            .ok_or_else(|| StartupError::Other("no parent directory".into()))?;

        let script = format!(
            "$s=(New-Object -ComObject WScript.Shell).CreateShortcut('{}');\
             $s.TargetPath='{}';$s.WorkingDirectory='{}';$s.Save()",
            link.display(),
            exe.display(),
            dir.display()
        );

        let output = std::process::Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .output()?;

        if output.status.success() {
            Ok(())
        } else {
            Err(StartupError::Other(
                String::from_utf8_lossy(&output.stderr).trim().to_string(),
            ))
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use super::StartupError;
    use std::sync::atomic::{AtomicBool, Ordering};

    static ENABLED: AtomicBool = AtomicBool::new(false);

    pub fn is_enabled() -> bool {
        ENABLED.load(Ordering::Relaxed)
    }

    /// No-op on non-Windows so the Settings toggle is still exercisable.
    pub fn set_enabled(enabled: bool) -> Result<(), StartupError> {
        ENABLED.store(enabled, Ordering::Relaxed);
        Ok(())
    }
}

pub use imp::{is_enabled, set_enabled};
```

- [ ] **Step 3: Write the settings view**

Replace `src/ui/settings.rs`:

```rust
use dioxus::prelude::*;

use crate::app::tool_path;
use crate::deps::{self, ToolStatus};
use crate::startup;
use crate::ui::{AppState, Handles};

#[component]
pub fn Settings() -> Element {
    let state = use_context::<AppState>();
    let handles = use_context::<Handles>();

    let mut config = state.config;
    let mut busy = use_signal(|| false);
    let mut message = use_signal(String::new);
    let mut at_startup = use_signal(startup::is_enabled);

    let cfg = config();
    let tool = tool_path(&cfg, &handles.dir);
    let found = deps::status(&tool) == ToolStatus::Found;

    rsx! {
        h1 { "Settings" }

        div { class: "card",
            h2 { "ControlMyMonitor" }
            p {
                if found {
                    span { style: "color:var(--ok)", "Found: " }
                } else {
                    span { style: "color:var(--err)", "Missing: " }
                }
                span { style: "font-family:monospace; font-size:12px;", "{tool.display()}" }
            }

            div { style: "display:flex; gap:8px; align-items:center; flex-wrap:wrap;",
                button {
                    class: "primary",
                    disabled: busy(),
                    onclick: {
                        let handles = handles.clone();
                        move |_| {
                            busy.set(true);
                            message.set("Downloading from nirsoft.net…".into());
                            let dir = handles.dir.clone();
                            let mut state = state;
                            spawn(async move {
                                let result = deps::download_to(&dir);
                                busy.set(false);
                                match result {
                                    Ok(()) => {
                                        message.set("ControlMyMonitor.exe installed.".into());
                                        state.tool_ok.set(true);
                                    }
                                    Err(e) => message.set(format!("Download failed: {e}")),
                                }
                            });
                        }
                    },
                    if busy() { "Downloading…" } else { "Download from NirSoft" }
                }
            }

            div { style: "margin-top:10px;",
                div { style: "color:var(--text-dim); margin-bottom:4px;",
                    "Or point at an existing copy:"
                }
                input {
                    r#type: "text",
                    style: "width:100%",
                    placeholder: r"C:\Tools\ControlMyMonitor.exe",
                    value: "{cfg.control_my_monitor_path}",
                    onchange: {
                        let handles = handles.clone();
                        move |e: Event<FormData>| {
                            let mut c = config();
                            c.control_my_monitor_path = e.value();
                            handles.save(&c);
                            config.set(c);
                        }
                    },
                }
            }

            if !message().is_empty() {
                p { style: "color:var(--text-dim)", "{message}" }
            }

            p { style: "color:var(--text-dim); font-size:12px; margin-bottom:0;",
                "ControlMyMonitor is a free utility by NirSoft — nirsoft.net/utils/control_my_monitor.html"
            }
        }

        div { class: "card",
            h2 { "Behaviour" }

            label { style: "display:flex; gap:8px; align-items:center; margin-bottom:14px;",
                input {
                    r#type: "checkbox",
                    checked: at_startup(),
                    onchange: move |e| {
                        let want = e.checked();
                        match startup::set_enabled(want) {
                            Ok(()) => at_startup.set(want),
                            Err(err) => message.set(format!("Could not change startup: {err}")),
                        }
                    },
                }
                "Run at startup"
            }

            div {
                div { style: "color:var(--text-dim); margin-bottom:6px;",
                    "Switch cooldown: {cfg.cooldown_secs}s"
                }
                input {
                    r#type: "range", min: "2", max: "300", step: "1",
                    style: "width:320px",
                    value: "{cfg.cooldown_secs}",
                    oninput: {
                        let handles = handles.clone();
                        move |e: Event<FormData>| {
                            if let Ok(v) = e.value().parse::<u64>() {
                                let mut c = config();
                                c.cooldown_secs = v;
                                handles.save(&c);
                                config.set(c);
                            }
                        }
                    },
                }
                p { style: "color:var(--text-dim); font-size:12px;",
                    "After a switch, further switches are suppressed for this long. If the
                     KVM changes during the cooldown, the monitors are resynced when it ends."
                }
            }
        }

        div { class: "card",
            h2 { "Files" }
            p { style: "font-family:monospace; font-size:12px;", "{handles.dir.display()}" }
        }
    }
}
```

- [ ] **Step 4: Register the new modules and run migration at startup**

In `src/lib.rs`, add `pub mod deps;` and `pub mod startup;`. In `src/main.rs`, add them to the `use monitor_switcher::{...}` list and replace the config load with a migration-aware version:

```rust
    let raw = std::fs::read_to_string(dir.join(config::CONFIG_FILE)).unwrap_or_default();

    let cfg = if !raw.is_empty() && config::is_v1(&raw) {
        // Migration needs a /stext dump to map display names to serials. If the
        // tool is not available the v1 file is left untouched and retried next
        // launch — a config is never partially migrated.
        let probe = dir.join("ControlMyMonitor.exe");
        match hardware::list_monitors(&probe) {
            Ok(monitors) => {
                let stext: Vec<_> = monitors
                    .iter()
                    .map(|m| hardware::parse::StextMonitor {
                        device_name: m.device_name.clone(),
                        model: m.model.clone(),
                        serial: m.serial.clone(),
                        current_input: m.current_input,
                    })
                    .collect();

                let present = hardware::list_devices().unwrap_or_default();
                let names = |id: &str| {
                    present
                        .iter()
                        .find(|d| d.id == id)
                        .map(|d| (d.name.clone(), d.class.as_str().to_string()))
                };

                match Config::migrate_v1(&raw, &stext, &names) {
                    Ok(migrated) => {
                        let _ = std::fs::copy(
                            dir.join(config::CONFIG_FILE),
                            dir.join(format!("{}.bak", config::CONFIG_FILE)),
                        );
                        migrated.save(&dir)?;
                        migrated
                    }
                    Err(_) => Config::load(&dir)?,
                }
            }
            Err(_) => Config::default(),
        }
    } else {
        Config::load(&dir)?
    };
```

- [ ] **Step 5: Verify**

Run: `cargo run` on Linux — the Settings tab renders, the cooldown slider persists, and the startup toggle flips without error. Download is expected to work (it hits the real NirSoft URL); if you would rather not, confirm it at least reports a sensible error.

On Windows: place an old-format `config.toml` next to the exe, run, and confirm `config.toml.bak` appears and the rules show up under Monitors with real serials.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml Cargo.lock src/deps.rs src/startup.rs src/main.rs src/ui/settings.rs
git commit -m "feat: add settings, dependency download assistant and startup shortcut"
```

---

## Task 16: CI, documentation and cleanup

**Files:**
- Modify: `.github/workflows/rust.yml`, `README.md`
- Delete: `TODO.md`, `config.toml`

**Interfaces:**
- Consumes: everything
- Produces: nothing (final task)

- [ ] **Step 1: Add a Linux job to CI**

In `.github/workflows/rust.yml`, add a second job alongside `build` so the mock backend cannot silently drift:

```yaml
  linux-check:
    runs-on: ubuntu-latest
    steps:
    - uses: actions/checkout@v4

    - name: Set up Rust
      uses: dtolnay/rust-toolchain@stable
      with:
        toolchain: stable

    - name: Install Dioxus desktop system dependencies
      run: |
        sudo apt-get update
        sudo apt-get install -y libwebkit2gtk-4.1-dev libgtk-3-dev libsoup-3.0-dev \
                                libjavascriptcoregtk-4.1-dev pkg-config

    - name: Check
      run: cargo check --all-targets

    - name: Test
      run: cargo test
```

Also change the existing Windows `Package` step: `config.toml` is no longer copied by `build.rs`, so remove it from the `Compress-Archive` path list, leaving `target/release/monitor_switcher.exe` and `target/release/icon.ico`.

- [ ] **Step 2: Remove the superseded files**

```bash
git rm TODO.md config.toml
```

`TODO.md` is superseded by the design document; the sample `config.toml` is superseded by the generated default (and shipping one risks overwriting a real config on extract).

- [ ] **Step 3: Rewrite the README**

Replace the "Configuration" and "How to find VID and PID" sections of `README.md` with:

```markdown
## Setup

1. Run `monitor_switcher.exe`. The window opens on first launch.
2. **Settings** → download `ControlMyMonitor.exe` from NirSoft, or point at an existing copy.
3. **Devices** → tick the devices that move with your KVM. Unplug the KVM and watch the
   list to see which entries disappear — those are the ones to tick.
4. **Monitors** → click each screen and choose the input it should show when the KVM is
   connected and when it is not. Use **Test** if you are unsure which input number is which:
   the monitor switches for eight seconds, switches back on its own, and then asks whether
   to keep the value.
5. Close the window. The app keeps running in the tray. Quit from the tray menu.

Settings are saved to `config.toml` next to the executable as you change them. A config from
an older version is migrated automatically on first launch, and the original is kept as
`config.toml.bak`.

## Development

The app targets Windows but builds and runs on Linux against a mock hardware backend, chosen
automatically by target — no feature flag. `cargo run` on Linux opens the full UI with canned
devices and monitors, plus a **Debug** tab that simulates device connect and disconnect
events. Requires `webkit2gtk` and `libsoup`.

`cargo test` runs everywhere: the config migration, the `/stext` parser, and the watcher
state machine are pure and platform-independent.
```

Keep the existing NirSoft attribution and the PowerShell snippets — they remain useful for
manual diagnosis.

- [ ] **Step 4: Verify the whole thing**

Run on Linux: `cargo check --all-targets && cargo test && cargo run`
Run on Windows: `cargo build --release`, then exercise a real KVM toggle end to end.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "chore: add Linux CI job, refresh the README and drop superseded files"
```

---

## Self-Review Notes

Checked against the spec:

- §2 decisions — every row maps to a task (tray-first: 9; ControlMyMonitor: 6; event-driven: 8; no tokio: 8; SetupAPI: 5; read-only layout: 13; serial keys: 1; migration: 3; plain CSS: 9; no command editing: 13/14; Linux mock: 4/11).
- §4 config — Task 1 (schema) and Task 3 (migration, including the tool-missing deferral wired up in Task 15 step 4).
- §5 hardware — Tasks 2, 5, 6.
- §6 state machine — Task 7, with the debounce living in the thread loop in Task 8.
- §7 UI — Tasks 10 (dashboard), 12 (devices), 13/14 (monitors), 15 (settings).
- §8 errors — corruption recovery in Task 1, tool-missing banners in Tasks 10/15, command failures logged in Task 8's `apply_all`.
- §9 testing — Tasks 1, 2, 3, 4, 7, 13 carry tests.
- §11 Linux development — Tasks 4, 11, 16.
- §12 startup — Task 15.

Known soft spots for the implementer to resolve against live docs rather than guesswork:

1. **Dioxus 0.7 desktop specifics** — the hide-on-close hook and `LaunchBuilder` shape (Task 9), and the one-second timer inside the test countdown (Task 14). The plan states the required behaviour; the exact API may differ.
2. **`windows` 0.52 SetupAPI signatures** (Task 5) — buffer parameters are the usual mismatch. The enumeration structure is what matters.
3. **`tray-icon` on the Dioxus event loop** (Task 9) — if the tray refuses to build inside `use_effect`, move it into a custom event handler on the desktop config.
