//! Fetching ControlMyMonitor, which the app needs but doesn't ship.

use std::path::Path;

const DOWNLOAD_URL: &str = "https://www.nirsoft.net/utils/controlmymonitor.zip";
pub const EXE_NAME: &str = "ControlMyMonitor.exe";

#[derive(Debug, thiserror::Error)]
pub enum DepsError {
    #[error("{0}")]
    Failed(String),
    #[error("{0}")]
    Io(#[from] std::io::Error),
}

pub fn is_present(tool: &Path) -> bool {
    tool.is_file()
}

/// Quotes `s` as a PowerShell single-quoted string literal.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn ps_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// Downloads NirSoft's zip and extracts ControlMyMonitor.exe into `dir`.
///
/// ControlMyMonitor is NirSoft's work, fetched from their own site at the
/// user's request rather than redistributed with the app.
// ponytail: PowerShell's Invoke-WebRequest and .NET ZipFile instead of HTTP and
// zip crates; this is Windows-only software anyway, and it's one call.
#[cfg(windows)]
pub fn download_to(dir: &Path) -> Result<(), DepsError> {
    let zip = std::env::temp_dir().join(format!("controlmymonitor_{}.zip", std::process::id()));
    let script = format!(
        "$ErrorActionPreference='Stop'; $zip={zip}; \
         Invoke-WebRequest -UseBasicParsing -Uri {url} -OutFile $zip; \
         Add-Type -AssemblyName System.IO.Compression.FileSystem; \
         $z=[IO.Compression.ZipFile]::OpenRead($zip); \
         try {{ $e=$z.Entries | Where-Object Name -eq {exe} | Select-Object -First 1; \
               if (-not $e) {{ throw 'the archive did not contain {EXE_NAME}' }}; \
               [IO.Compression.ZipFileExtensions]::ExtractToFile($e, {dest}, $true) }} \
         finally {{ $z.Dispose(); Remove-Item $zip -ErrorAction SilentlyContinue }}",
        zip = ps_quote(&zip.display().to_string()),
        url = ps_quote(DOWNLOAD_URL),
        exe = ps_quote(EXE_NAME),
        dest = ps_quote(&dir.join(EXE_NAME).display().to_string()),
    );

    let output = std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .output()?;

    if output.status.success() {
        Ok(())
    } else {
        Err(DepsError::Failed(String::from_utf8_lossy(&output.stderr).trim().to_string()))
    }
}

#[cfg(not(windows))]
pub fn download_to(_dir: &Path) -> Result<(), DepsError> {
    Err(DepsError::Failed("ControlMyMonitor is Windows-only".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ps_quote_escapes_embedded_single_quotes() {
        assert_eq!(ps_quote(r"C:\Users\O'Brien\x.exe"), r"'C:\Users\O''Brien\x.exe'");
    }
}
