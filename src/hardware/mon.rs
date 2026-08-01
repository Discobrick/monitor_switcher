//! Monitor discovery and control.
//!
//! Geometry comes from GDI (`EnumDisplayMonitors`); identity and current input
//! come from a ControlMyMonitor `/stext` dump. The two are correlated on the
//! `\\.\DISPLAYn` prefix, which both sides report.
//!
//! Everything here that is not Win32 FFI or process spawning lives in the
//! parent module, where it is testable on any platform.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;

use windows::Win32::Foundation::{BOOL, LPARAM, RECT};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO, MONITORINFOEXW,
};
use windows::Win32::UI::WindowsAndMessaging::MONITORINFOF_PRIMARY;

use super::parse::{parse_stext, StextMonitor};
use super::{combine, decode_dump, input_for_serial, tool_error, HardwareError, MonitorGeometry, MonitorInfo};

/// DDC/CI input-select VCP code. Fixed by the MCCS standard, so it is never
/// configurable.
const VCP_INPUT_SELECT: &str = "60";

pub fn list_monitors(tool: &Path) -> Result<Vec<MonitorInfo>, HardwareError> {
    Ok(combine(dump_stext(tool)?, &enumerate_gdi()))
}

pub fn read_input(tool: &Path, serial: &str) -> Result<u16, HardwareError> {
    input_for_serial(&dump_stext(tool)?, serial)
}

/// Sets VCP 60 on the monitor with the given serial.
///
/// ControlMyMonitor accepts a serial number directly as its monitor argument,
/// so no display-index lookup is needed at switch time — which matters, because
/// display indices shuffle on replug and reboot while serials do not.
pub fn apply_input(tool: &Path, serial: &str, value: u16) -> Result<(), HardwareError> {
    let value = value.to_string();
    let args = ["/SetValue", serial, VCP_INPUT_SELECT, &value].map(OsStr::new);
    let output = run(tool, &args)?;

    if output.status.success() {
        Ok(())
    } else {
        Err(tool_error(output.status.code(), &output.stderr))
    }
}

/// Removes the dump file on every exit path, including the error returns.
struct TempDump(PathBuf);

impl Drop for TempDump {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Runs `/stext` into a temp file and parses it.
fn dump_stext(tool: &Path) -> Result<Vec<StextMonitor>, HardwareError> {
    // Named per process so two instances cannot read each other's dump.
    let dump = TempDump(std::env::temp_dir().join(format!("monsw_{}.txt", std::process::id())));

    let output = run(tool, &[OsStr::new("/stext"), dump.0.as_os_str()])?;

    if !output.status.success() {
        return Err(tool_error(output.status.code(), &output.stderr));
    }

    Ok(parse_stext(&decode_dump(&std::fs::read(&dump.0)?)))
}

/// Spawns the tool, reporting a missing executable as such rather than as a
/// bare `NotFound` io error.
fn run(tool: &Path, args: &[&OsStr]) -> Result<std::process::Output, HardwareError> {
    Command::new(tool).args(args).output().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            HardwareError::ToolMissing(tool.display().to_string())
        } else {
            HardwareError::Io(e)
        }
    })
}

fn enumerate_gdi() -> Vec<MonitorGeometry> {
    let mut result: Vec<MonitorGeometry> = Vec::new();

    // SAFETY: the callback receives a pointer to `result` as its LPARAM and is
    // only invoked for the duration of this call, during which `result` is live.
    unsafe {
        EnumDisplayMonitors(
            HDC::default(),
            None,
            Some(enum_proc),
            LPARAM(&mut result as *mut Vec<MonitorGeometry> as isize),
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
        monitorInfo: MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFOEXW>() as u32,
            ..Default::default()
        },
        ..Default::default()
    };

    // SAFETY: cbSize is set to the extended struct size, which is how
    // GetMonitorInfoW is told the buffer is a MONITORINFOEXW.
    let ok = unsafe { GetMonitorInfoW(hmonitor, &mut info as *mut MONITORINFOEXW as *mut MONITORINFO) };

    // A monitor that cannot be described is skipped, not fatal: the dump still
    // lists it, and `combine` falls back to a default rectangle. Returning TRUE
    // keeps the remaining monitors coming.
    if !ok.as_bool() {
        return BOOL(1);
    }

    let end = info
        .szDevice
        .iter()
        .position(|&c| c == 0)
        .unwrap_or(info.szDevice.len());
    let rect = info.monitorInfo.rcMonitor;

    // SAFETY: lparam carries the &mut Vec passed by enumerate_gdi, which
    // outlives this callback.
    let out = unsafe { &mut *(lparam.0 as *mut Vec<MonitorGeometry>) };
    out.push(MonitorGeometry {
        device: String::from_utf16_lossy(&info.szDevice[..end]),
        x: rect.left,
        y: rect.top,
        width: rect.right - rect.left,
        height: rect.bottom - rect.top,
        is_primary: info.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0,
    });

    BOOL(1)
}
