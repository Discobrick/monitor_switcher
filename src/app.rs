//! Application-level glue: where things live on disk, and the message types
//! the watcher thread and the UI exchange.

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
    /// Some USB device came or went; views listing devices should re-enumerate.
    DevicesChanged,
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
        let cfg = Config { control_my_monitor_path: "   ".into(), ..Config::default() };
        assert_eq!(
            tool_path(&cfg, Path::new("/opt/msw")),
            PathBuf::from("/opt/msw/ControlMyMonitor.exe")
        );
    }

    #[test]
    fn a_configured_override_wins_over_the_exe_directory() {
        let cfg = Config {
            control_my_monitor_path: r"D:\tools\ControlMyMonitor.exe".into(),
            ..Config::default()
        };
        assert_eq!(
            tool_path(&cfg, Path::new("/opt/msw")),
            PathBuf::from(r"D:\tools\ControlMyMonitor.exe")
        );
    }
}
