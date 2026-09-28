//! Fetching ControlMyMonitor, which the app needs but doesn't ship.

use std::io::{Cursor, Read};
use std::path::Path;

const DOWNLOAD_URL: &str = "https://www.nirsoft.net/utils/controlmymonitor.zip";
pub const EXE_NAME: &str = "ControlMyMonitor.exe";

#[derive(Debug, thiserror::Error)]
pub enum DepsError {
    #[error("download failed: {0}")]
    Download(String),
    #[error("could not read the archive: {0}")]
    Archive(#[from] zip::result::ZipError),
    #[error("the archive did not contain {EXE_NAME}")]
    NotInArchive,
    #[error("{0}")]
    Io(#[from] std::io::Error),
}

pub fn is_present(tool: &Path) -> bool {
    tool.is_file()
}

/// Downloads NirSoft's zip and extracts ControlMyMonitor.exe into `dir`.
///
/// ControlMyMonitor is NirSoft's work, fetched from their own site at the
/// user's request rather than redistributed with the app.
pub fn download_to(dir: &Path) -> Result<(), DepsError> {
    let mut body = Vec::new();
    ureq::get(DOWNLOAD_URL)
        .call()
        .map_err(|e| DepsError::Download(e.to_string()))?
        .into_body()
        .into_reader()
        .read_to_end(&mut body)?;
    extract_exe(&body, dir)
}

/// Writes the archive's ControlMyMonitor.exe (matched by file name, any
/// folder, any case) into `dir`, ignoring everything else in it.
fn extract_exe(zip_bytes: &[u8], dir: &Path) -> Result<(), DepsError> {
    let mut archive = zip::ZipArchive::new(Cursor::new(zip_bytes))?;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        let is_exe = entry
            .enclosed_name()
            .and_then(|p| p.file_name().map(|f| f.eq_ignore_ascii_case(EXE_NAME)))
            .unwrap_or(false);
        if is_exe {
            // Write aside and rename, so a failed write never leaves a
            // truncated exe where the app will try to run it.
            let partial = dir.join(format!("{EXE_NAME}.part"));
            std::io::copy(&mut entry, &mut std::fs::File::create(&partial)?)?;
            std::fs::rename(&partial, dir.join(EXE_NAME))?;
            return Ok(());
        }
    }
    Err(DepsError::NotInArchive)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use zip::write::SimpleFileOptions;

    fn zip_of(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        for (name, body) in files {
            w.start_file(*name, opts).unwrap();
            w.write_all(body).unwrap();
        }
        w.finish().unwrap().into_inner()
    }

    #[test]
    fn extracts_only_the_exe() {
        let dir = tempfile::tempdir().unwrap();
        let zip = zip_of(&[("readme.txt", b"hi"), ("ControlMyMonitor.exe", b"MZ-exe")]);

        extract_exe(&zip, dir.path()).unwrap();

        assert_eq!(std::fs::read(dir.path().join(EXE_NAME)).unwrap(), b"MZ-exe");
        assert!(!dir.path().join("readme.txt").exists());
        assert!(!dir.path().join(format!("{EXE_NAME}.part")).exists());
    }

    #[test]
    fn an_archive_without_the_exe_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let zip = zip_of(&[("readme.txt", b"hi")]);
        assert!(matches!(extract_exe(&zip, dir.path()), Err(DepsError::NotInArchive)));
    }

    #[test]
    fn garbage_is_an_archive_error_not_a_panic() {
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(extract_exe(b"<html>blocked</html>", dir.path()), Err(DepsError::Archive(_))));
    }
}
