//! Linux development backend: stubs filled in by a later task.
#![allow(dead_code)]

use super::{HardwareError, MonitorInfo, UsbDevice};

pub fn list_monitors() -> Result<Vec<MonitorInfo>, HardwareError> {
    unimplemented!("filled in by a later task")
}

pub fn list_devices() -> Result<Vec<UsbDevice>, HardwareError> {
    unimplemented!("filled in by a later task")
}

pub fn read_input(_serial: &str) -> Result<u16, HardwareError> {
    unimplemented!("filled in by a later task")
}

pub fn apply_input(_serial: &str, _input: u16) -> Result<(), HardwareError> {
    unimplemented!("filled in by a later task")
}
