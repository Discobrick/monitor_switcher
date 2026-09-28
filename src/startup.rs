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
    use crate::deps::ps_quote;

    const LINK_NAME: &str = "Monitor Switcher.lnk";

    fn startup_dir() -> Result<PathBuf, StartupError> {
        let appdata = std::env::var("APPDATA")
            .map_err(|_| StartupError::Other("APPDATA is not set".into()))?;
        Ok(PathBuf::from(appdata).join(r"Microsoft\Windows\Start Menu\Programs\Startup"))
    }

    pub fn is_enabled() -> bool {
        startup_dir().map(|d| d.join(LINK_NAME).exists()).unwrap_or(false)
    }

    /// Creates or removes the shortcut.
    // ponytail: shells out to PowerShell's WScript.Shell rather than pulling in
    // a COM/IShellLink binding for one .lnk.
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
            "$s=(New-Object -ComObject WScript.Shell).CreateShortcut({});\
             $s.TargetPath={};$s.WorkingDirectory={};$s.Save()",
            ps_quote(&link.display().to_string()),
            ps_quote(&exe.display().to_string()),
            ps_quote(&dir.display().to_string()),
        );

        let output = std::process::Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .output()?;

        if output.status.success() {
            Ok(())
        } else {
            Err(StartupError::Other(String::from_utf8_lossy(&output.stderr).trim().to_string()))
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
