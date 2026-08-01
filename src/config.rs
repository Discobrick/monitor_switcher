// ponytail: nothing constructs this module yet (GUI lands in a later task) —
// drop this allow once main.rs wires up Config.
#![allow(dead_code)]

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
