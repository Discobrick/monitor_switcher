//! Compile-time stand-in for the Win32 backend, used on non-Windows targets so
//! the UI can be developed and debugged on Linux. Never compiled on Windows.

use std::path::Path;
use std::sync::{Mutex, OnceLock};

use super::{DeviceClass, HardwareError, MonitorInfo, UsbDevice};

struct MockState {
    devices: Vec<(UsbDevice, bool)>,
    monitors: Vec<MonitorInfo>,
    tool_present: bool,
    fail_next: bool,
}

fn state() -> &'static Mutex<MockState> {
    static STATE: OnceLock<Mutex<MockState>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(MockState::new()))
}

impl MockState {
    fn new() -> Self {
        let dev = |id: &str, name: &str, class: DeviceClass| {
            (UsbDevice { id: id.into(), name: name.into(), class }, true)
        };

        // Eleven entries mirroring a real machine: a mix of properly named and
        // generic devices across several classes. The picker's deduplication
        // runs before this list is built, so entries here are already unique.
        let devices = vec![
            dev("VID_046D&PID_085C", "Logitech StreamCam", DeviceClass::Camera),
            dev("VID_046D&PID_C08B", "Logitech G502 HERO Gaming Mouse", DeviceClass::Mouse),
            dev("VID_3142&PID_0686", "USB Input Device", DeviceClass::Hid),
            dev("VID_1B1C&PID_1B7C", "Corsair K70 RGB Keyboard", DeviceClass::Keyboard),
            dev("VID_8087&PID_0032", "Intel Wireless Bluetooth", DeviceClass::Other),
            dev("VID_0BDA&PID_8153", "USB 10/100/1000 LAN", DeviceClass::Other),
            dev("VID_145F&PID_02A2", "USB Input Device", DeviceClass::Hid),
            dev("VID_05E3&PID_0610", "USB2.0 Hub", DeviceClass::Other),
            dev("VID_1462&PID_7C95", "USB Input Device", DeviceClass::Hid),
            dev("VID_0C45&PID_6366", "Trust Webcam", DeviceClass::Camera),
            dev("VID_04D9&PID_A052", "HID Keyboard Device", DeviceClass::Keyboard),
        ];

        // An L arrangement with a portrait secondary and a differently scaled
        // third screen, so the layout maths is genuinely exercised.
        let monitors = vec![
            MonitorInfo {
                serial: "ABC123456".into(),
                model: "DELL U2720Q".into(),
                device_name: r"\\.\DISPLAY1\Monitor0".into(),
                x: 0, y: 0, width: 3840, height: 2160,
                is_primary: true,
                current_input: Some(15),
            },
            MonitorInfo {
                serial: "XYZ987654".into(),
                model: "LG HDR 4K".into(),
                device_name: r"\\.\DISPLAY2\Monitor0".into(),
                x: 3840, y: -400, width: 1080, height: 1920,
                is_primary: false,
                current_input: Some(17),
            },
            MonitorInfo {
                serial: "QWE555111".into(),
                model: "AOC 24G2".into(),
                device_name: r"\\.\DISPLAY3\Monitor0".into(),
                x: -1920, y: 300, width: 1920, height: 1080,
                is_primary: false,
                current_input: Some(17),
            },
        ];

        Self { devices, monitors, tool_present: true, fail_next: false }
    }
}

pub fn list_devices() -> Result<Vec<UsbDevice>, HardwareError> {
    let s = state().lock().expect("mock state poisoned");
    Ok(s.devices
        .iter()
        .filter(|(_, present)| *present)
        .map(|(d, _)| d.clone())
        .collect())
}

pub fn list_monitors(_tool: &Path) -> Result<Vec<MonitorInfo>, HardwareError> {
    let s = state().lock().expect("mock state poisoned");
    if !s.tool_present {
        return Err(HardwareError::ToolMissing("ControlMyMonitor.exe".into()));
    }
    Ok(s.monitors.clone())
}

pub fn read_input(_tool: &Path, serial: &str) -> Result<u16, HardwareError> {
    let s = state().lock().expect("mock state poisoned");
    s.monitors
        .iter()
        .find(|m| m.serial == serial)
        .and_then(|m| m.current_input)
        .ok_or_else(|| HardwareError::UnknownSerial(serial.to_string()))
}

pub fn apply_input(_tool: &Path, serial: &str, value: u16) -> Result<(), HardwareError> {
    let mut s = state().lock().expect("mock state poisoned");

    if s.fail_next {
        s.fail_next = false;
        return Err(HardwareError::ToolFailed(
            "simulated failure: the monitor did not acknowledge the VCP write".into(),
        ));
    }
    if !s.tool_present {
        return Err(HardwareError::ToolMissing("ControlMyMonitor.exe".into()));
    }

    let m = s
        .monitors
        .iter_mut()
        .find(|m| m.serial == serial)
        .ok_or_else(|| HardwareError::UnknownSerial(serial.to_string()))?;
    m.current_input = Some(value);
    Ok(())
}

// -- controls driven by the debug panel ------------------------------------

pub fn set_device_present(id: &str, present: bool) {
    let mut s = state().lock().expect("mock state poisoned");
    if let Some((_, flag)) = s.devices.iter_mut().find(|(d, _)| d.id == id) {
        *flag = present;
    }
}

/// Unplugs or replugs every device at once — the KVM-toggle simulation.
pub fn set_all_present(present: bool) {
    let mut s = state().lock().expect("mock state poisoned");
    for (_, flag) in s.devices.iter_mut() {
        *flag = present;
    }
}

pub fn set_tool_present(present: bool) {
    state().lock().expect("mock state poisoned").tool_present = present;
}

pub fn set_fail_next_command(fail: bool) {
    state().lock().expect("mock state poisoned").fail_next = fail;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn devices_are_deduplicated_by_vid_pid() {
        let devices = list_devices().unwrap();
        let mut ids: Vec<&str> = devices.iter().map(|d| d.id.as_str()).collect();
        ids.sort_unstable();
        let before = ids.len();
        ids.dedup();
        assert_eq!(before, ids.len(), "mock returned duplicate VID&PID entries");
    }

    #[test]
    fn unplugging_removes_a_device_from_the_list() {
        set_device_present("VID_046D&PID_085C", false);
        assert!(list_devices().unwrap().iter().all(|d| d.id != "VID_046D&PID_085C"));

        set_device_present("VID_046D&PID_085C", true);
        assert!(list_devices().unwrap().iter().any(|d| d.id == "VID_046D&PID_085C"));
    }

    #[test]
    fn applying_an_input_is_visible_to_the_next_read() {
        let tool = std::path::Path::new("ControlMyMonitor.exe");
        apply_input(tool, "ABC123456", 18).unwrap();
        assert_eq!(read_input(tool, "ABC123456").unwrap(), 18);
    }

    #[test]
    fn forced_failure_surfaces_as_tool_failed() {
        let tool = std::path::Path::new("ControlMyMonitor.exe");
        set_fail_next_command(true);
        let err = apply_input(tool, "ABC123456", 15).unwrap_err();
        assert!(matches!(err, HardwareError::ToolFailed(_)));
    }
}
