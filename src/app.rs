//! Application-level glue: where things live on disk, and the message types
//! the watcher thread and the UI exchange.

use std::path::{Path, PathBuf};

use crate::config::{self, Config, ConfigError};
use crate::hardware::{self, MonitorInfo, parse::StextMonitor};

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

/// The identity half of a monitor enumeration, which is all `migrate_v1` needs.
///
/// Pure so the v1 upgrade path can be exercised without a `/stext` dump.
pub fn identities(monitors: &[MonitorInfo]) -> Vec<StextMonitor> {
    monitors
        .iter()
        .map(|m| StextMonitor {
            device_name: m.device_name.clone(),
            model: m.model.clone(),
            serial: m.serial.clone(),
            current_input: m.current_input,
        })
        .collect()
}

/// Loads the config, upgrading a pre-versioning file on the way through.
///
/// Migration is best-effort by design: it needs live hardware to resolve
/// display names to serials, and a machine where ControlMyMonitor is missing
/// must still start. Rules it cannot resolve are kept with an empty serial and
/// surfaced later as "re-detect this monitor" rather than silently dropped.
///
/// The v1 file is renamed aside before the v2 one is written, so a bad
/// migration is recoverable by hand.
pub fn load_config(dir: &Path) -> Result<Config, ConfigError> {
    let path = dir.join(config::CONFIG_FILE);
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return Config::load(dir);
    };
    if !config::is_v1(&raw) {
        return Config::load(dir);
    }

    let monitors = hardware::list_monitors(&dir.join("ControlMyMonitor.exe")).unwrap_or_default();
    let devices = hardware::list_devices().unwrap_or_default();
    let names = |id: &str| {
        devices
            .iter()
            .find(|d| d.id == id)
            .map(|d| (d.name.clone(), d.class.as_str().to_string()))
    };

    let Ok(migrated) = Config::migrate_v1(&raw, &identities(&monitors), &names) else {
        // Not actually a v1 config, just a file with no `version` line.
        // Config::load quarantines it and starts fresh.
        return Config::load(dir);
    };

    let _ = std::fs::rename(&path, dir.join(format!("{}.v1", config::CONFIG_FILE)));
    migrated.save(dir)?;
    Ok(migrated)
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
    /// Time of day, UTC, formatted `HH:MM:SS` for display. See
    /// `watcher::now_string`.
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
    /// Configuration changed; re-read the cooldown and the watch list.
    ConfigChanged,
    SetMonitoring(bool),
    /// Force a presence re-check (used by the Refresh button and the debug panel).
    Poke,
    Shutdown,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DeviceEntry;

    #[test]
    fn an_empty_tool_path_resolves_next_to_the_executable() {
        let cfg = Config::default();
        assert_eq!(
            tool_path(&cfg, Path::new("/opt/msw")),
            PathBuf::from("/opt/msw/ControlMyMonitor.exe")
        );
    }

    /// Whitespace is not a configured path — a user who clears the field in the
    /// UI must fall back, not get a spawn of "  ".
    #[test]
    fn a_blank_override_is_treated_as_unset() {
        let mut cfg = Config::default();
        cfg.control_my_monitor_path = "   ".into();
        assert_eq!(
            tool_path(&cfg, Path::new("/opt/msw")),
            PathBuf::from("/opt/msw/ControlMyMonitor.exe")
        );
    }

    #[test]
    fn a_configured_override_wins_over_the_exe_directory() {
        let mut cfg = Config::default();
        cfg.control_my_monitor_path = r"D:\tools\ControlMyMonitor.exe".into();
        assert_eq!(
            tool_path(&cfg, Path::new("/opt/msw")),
            PathBuf::from(r"D:\tools\ControlMyMonitor.exe")
        );
    }

    #[test]
    fn identities_carry_the_serial_and_input_migration_needs() {
        let monitors = vec![MonitorInfo {
            serial: "ABC123".into(),
            model: "DELL U2720Q".into(),
            device_name: r"\\.\DISPLAY1\Monitor0".into(),
            x: 0,
            y: 0,
            width: 3840,
            height: 2160,
            is_primary: true,
            current_input: Some(15),
        }];

        let out = identities(&monitors);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].serial, "ABC123");
        assert_eq!(out[0].device_name, r"\\.\DISPLAY1\Monitor0");
        assert_eq!(out[0].current_input, Some(15));
    }

    #[test]
    fn a_v2_config_loads_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = Config::default();
        cfg.cooldown_secs = 90;
        cfg.devices.push(DeviceEntry {
            id: "VID_046D&PID_085C".into(),
            name: "Logitech".into(),
            class: "Mouse".into(),
        });
        cfg.save(dir.path()).unwrap();

        let loaded = load_config(dir.path()).unwrap();
        assert_eq!(loaded.cooldown_secs, 90);
        assert_eq!(loaded.devices.len(), 1);
        assert!(!dir.path().join("config.toml.v1").exists());
    }

    #[test]
    fn a_missing_config_yields_defaults() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load_config(dir.path()).unwrap().cooldown_secs, 60);
    }

    /// The upgrade must happen on load, not on the next save: a user who never
    /// opens the settings window still ends up on v2.
    #[test]
    fn a_v1_config_is_migrated_and_the_original_kept() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("config.toml"),
            "monitored_devices = [\"VID_046D&PID_085C\"]\n\
             connect_cmds = ['/SetValue \"\\\\\\\\.\\\\Display1\\\\Monitor0\" 60 15']\n\
             disconnect_cmds = ['/SetValue \"\\\\\\\\.\\\\Display1\\\\Monitor0\" 60 17']\n",
        )
        .unwrap();

        let migrated = load_config(dir.path()).unwrap();

        assert_eq!(migrated.version, 2);
        assert_eq!(migrated.devices.len(), 1);
        assert_eq!(migrated.monitors.len(), 1);
        assert_eq!(migrated.monitors[0].on_connect, 15);
        assert_eq!(migrated.monitors[0].on_disconnect, 17);

        // The original is kept, and the file on disk is now v2.
        assert!(dir.path().join("config.toml.v1").exists());
        let reloaded = load_config(dir.path()).unwrap();
        assert_eq!(reloaded.version, 2);
        assert_eq!(reloaded.monitors.len(), 1);
    }
}
