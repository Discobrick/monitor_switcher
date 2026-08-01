//! Win32 SetupAPI USB enumeration backend: filled in by a later task.
#![allow(dead_code)]

use super::{HardwareError, UsbDevice};

pub fn list_devices() -> Result<Vec<UsbDevice>, HardwareError> {
    unimplemented!("filled in by a later task")
}
