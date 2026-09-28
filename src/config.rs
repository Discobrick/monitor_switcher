
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
    /// PnP Monitor ID, e.g. `MONITOR\DELA28A\{...}\0001`. Display indices
    /// shuffle across reboots; this does not.
    pub monitor_id: String,
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
    /// How long an input test shows the candidate before switching back.
    #[serde(default = "default_test_secs")]
    pub test_secs: u32,
    pub monitoring_enabled: bool,
    /// Empty means "look next to the executable".
    pub control_my_monitor_path: String,
    /// Set once ControlMyMonitor is in place after first launch, so a missing
    /// tool is fetched automatically only until then. See `ui::first_launch`.
    #[serde(default)]
    pub setup_done: bool,
    #[serde(default)]
    pub devices: Vec<DeviceEntry>,
    #[serde(default)]
    pub monitors: Vec<MonitorRule>,
    /// User names for devices, keyed by id; any device can have one, watched or not.
    #[serde(default)]
    pub labels: std::collections::BTreeMap<String, String>,
}

/// Long enough for a monitor to resync and a sleeping PC on the other input
/// to wake.
fn default_test_secs() -> u32 {
    15
}

impl Default for Config {
    fn default() -> Self {
        Self {
            version: CURRENT_VERSION,
            cooldown_secs: 60,
            test_secs: default_test_secs(),
            monitoring_enabled: true,
            control_my_monitor_path: String::new(),
            setup_done: false,
            devices: Vec::new(),
            monitors: Vec::new(),
            labels: Default::default(),
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
                // Quarantine to a name nothing else can be sitting on, and
                // propagate a failed rename instead of falling through to
                // overwrite `path` with defaults. Either way the original
                // bytes survive, or the caller gets an Err and finds out.
                let broken = Self::quarantine_path(dir);
                std::fs::rename(&path, &broken)
                    .map_err(|source| ConfigError::Io { path: path.clone(), source })?;
                let c = Config::default();
                c.save(dir)?;
                Ok(c)
            }
        }
    }

    /// Picks `config.toml.broken`, or a timestamped variant if that name is
    /// already occupied by an earlier quarantine.
    fn quarantine_path(dir: &Path) -> PathBuf {
        let base = dir.join(format!("{CONFIG_FILE}.broken"));
        if !base.exists() {
            return base;
        }
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        dir.join(format!("{CONFIG_FILE}.broken.{nanos}"))
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

    /// There is no migration: an old command-string config is set aside like
    /// any other unreadable file, and the app starts fresh.
    #[test]
    fn a_v1_config_is_quarantined_not_migrated() {
        let dir = tempfile::tempdir().unwrap();
        let v1 = "monitored_devices = [\"VID_046D&PID_085C\"]\nconnect_cmds = []\n";
        std::fs::write(dir.path().join("config.toml"), v1).unwrap();

        let c = Config::load(dir.path()).unwrap();
        assert!(c.devices.is_empty());
        assert_eq!(std::fs::read_to_string(dir.path().join("config.toml.broken")).unwrap(), v1);
    }

    /// Configs written before `test_secs` existed must load, not be quarantined.
    #[test]
    fn a_config_without_test_secs_loads_with_the_default() {
        let dir = tempfile::tempdir().unwrap();
        let old = "version = 2\ncooldown_secs = 60\nmonitoring_enabled = true\ncontrol_my_monitor_path = \"\"\n";
        std::fs::write(dir.path().join("config.toml"), old).unwrap();

        assert_eq!(Config::load(dir.path()).unwrap().test_secs, 15);
        assert!(!dir.path().join("config.toml.broken").exists());
    }

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
            monitor_id: "ABC123456".into(),
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

    #[test]
    fn corrupt_file_survives_when_broken_name_already_taken() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.toml.broken"), "already broken").unwrap();
        std::fs::write(dir.path().join("config.toml"), "still bad toml").unwrap();

        let c = Config::load(dir.path()).unwrap();
        assert_eq!(c.cooldown_secs, 60);

        // The earlier quarantine file is untouched.
        assert_eq!(
            std::fs::read_to_string(dir.path().join("config.toml.broken")).unwrap(),
            "already broken"
        );

        // The newly-corrupt bytes are recoverable somewhere on disk, not
        // silently dropped by an overwrite.
        let recovered = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .find(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("config.toml.broken.")
            })
            .map(|e| std::fs::read_to_string(e.path()).unwrap());
        assert_eq!(recovered.as_deref(), Some("still bad toml"));
    }
}
