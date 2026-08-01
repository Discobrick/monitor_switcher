//! Win32 GDI + ControlMyMonitor backend: filled in by a later task.
#![allow(dead_code)]

use super::{HardwareError, MonitorInfo};

pub fn list_monitors() -> Result<Vec<MonitorInfo>, HardwareError> {
    unimplemented!("filled in by a later task")
}

pub fn read_input(_serial: &str) -> Result<u16, HardwareError> {
    unimplemented!("filled in by a later task")
}

pub fn apply_input(_serial: &str, _input: u16) -> Result<(), HardwareError> {
    unimplemented!("filled in by a later task")
}
