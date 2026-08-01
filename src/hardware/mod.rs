pub mod parse;

#[cfg(windows)]
mod mon;
#[cfg(windows)]
mod usb;

#[cfg(not(windows))]
mod mock;

#[cfg(windows)]
pub use mon::{apply_input, list_monitors, read_input};
#[cfg(windows)]
pub use usb::list_devices;

#[cfg(not(windows))]
pub use mock::{apply_input, list_devices, list_monitors, read_input};

/// Broad category used to pick the icon in the device picker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceClass {
    Mouse,
    Keyboard,
    Camera,
    Hid,
    Other,
}

impl DeviceClass {
    /// Maps a SetupAPI `SPDRP_CLASS` string onto a category.
    pub fn from_setup_class(class: &str) -> Self {
        match class.to_ascii_lowercase().as_str() {
            "mouse" => Self::Mouse,
            "keyboard" => Self::Keyboard,
            "camera" | "image" | "media" => Self::Camera,
            "hidclass" => Self::Hid,
            _ => Self::Other,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Mouse => "Mouse",
            Self::Keyboard => "Keyboard",
            Self::Camera => "Camera",
            Self::Hid => "HIDClass",
            Self::Other => "Other",
        }
    }
}

/// A physically present USB device, deduplicated by VID&PID.
#[derive(Debug, Clone, PartialEq)]
pub struct UsbDevice {
    /// "VID_XXXX&PID_YYYY"
    pub id: String,
    pub name: String,
    pub class: DeviceClass,
}

/// A monitor, merging GDI geometry with ControlMyMonitor identity.
#[derive(Debug, Clone, PartialEq)]
pub struct MonitorInfo {
    pub serial: String,
    pub model: String,
    /// `\\.\DISPLAY1\Monitor0`, resolved fresh each enumeration.
    pub device_name: String,
    /// Virtual-desktop position, for the layout diagram.
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub is_primary: bool,
    pub current_input: Option<u16>,
}

#[derive(Debug, thiserror::Error)]
pub enum HardwareError {
    #[error("ControlMyMonitor.exe not found at {0}")]
    ToolMissing(String),
    #[error("ControlMyMonitor.exe failed: {0}")]
    ToolFailed(String),
    #[error("no monitor with serial {0}")]
    UnknownSerial(String),
    #[error("{0}")]
    Io(#[from] std::io::Error),
}
