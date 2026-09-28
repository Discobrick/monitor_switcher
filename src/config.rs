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
    #[serde(default)]
    pub devices: Vec<DeviceEntry>,
    #[serde(default)]
    pub monitors: Vec<MonitorRule>,
    /// User names for devices, keyed by id; any device can have one, watched or not.
    #[serde(default)]
    pub labels: std::collections::BTreeMap<String, String>,
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

#[cfg(test)]
mod tests {
    use super::*;
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
