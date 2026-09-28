//! Monitor discovery and control.
//!
//! Geometry comes from GDI (`EnumDisplayMonitors`); identity comes from a
//! ControlMyMonitor `/smonitors` dump and the current input from `/GetValue`.
//! Geometry and identity are correlated on the `\\.\DISPLAYn` prefix, which
//! both sides report.
//!
//! Everything here that is not Win32 FFI or process spawning lives in the
//! parent module, where it is testable on any platform.

use std::ffi::OsStr;
use std::path::Path;
use std::process::Command;

use windows::Win32::Foundation::{BOOL, LPARAM, RECT};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO, MONITORINFOEXW,
};
use windows::Win32::UI::WindowsAndMessaging::MONITORINFOF_PRIMARY;

use super::parse::StextMonitor;
use super::{
    combine, input_from_exit_code, parse_dump, tool_error, HardwareError, MonitorGeometry, MonitorInfo,
    TempDump,
};

/// DDC/CI input-select VCP code. Fixed by the MCCS standard, so it is never
/// configurable.
const VCP_INPUT_SELECT: &str = "60";

pub fn list_monitors(tool: &Path) -> Result<Vec<MonitorInfo>, HardwareError> {
    let mut monitors = dump_smonitors(tool)?;
    for m in &mut monitors {
        // One ~150ms DDC read each; a monitor that won't answer just shows "?".
        m.current_input = read_input(tool, &m.serial).ok();
    }
    Ok(combine(monitors, &enumerate_gdi()))
}

/// Reads VCP 60 via `/GetValue`, which reports the value as its exit code.
pub fn read_input(tool: &Path, serial: &str) -> Result<u16, HardwareError> {
    let args = ["/GetValue", serial, VCP_INPUT_SELECT].map(OsStr::new);
    let output = run(tool, &args)?;
    output
        .status
        .code()
        .and_then(input_from_exit_code)
        .ok_or_else(|| HardwareError::UnknownSerial(serial.to_string()))
}

/// Sets VCP 60 on the monitor with the given key (its PnP Monitor ID).
///
/// ControlMyMonitor accepts the Monitor ID directly as its monitor argument,
/// so no display-index lookup is needed at switch time — which matters, because
/// display indices shuffle on replug and reboot while Monitor IDs do not.
/// (It does not accept serial numbers, despite what the field is called.)
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

/// Runs `/smonitors` into a temp file and parses it.
fn dump_smonitors(tool: &Path) -> Result<Vec<StextMonitor>, HardwareError> {
    let dump = TempDump::new();
    let output = run(tool, &[OsStr::new("/smonitors"), dump.path().as_os_str()])?;

    if !output.status.success() {
        return Err(tool_error(output.status.code(), &output.stderr));
    }

    parse_dump(&std::fs::read(dump.path())?)
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
