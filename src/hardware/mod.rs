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

/// Where a device node's name came from.
///
/// The ordering *is* the precedence rule: one physical device shows up as
/// several device nodes, and a node reporting a `SPDRP_FRIENDLYNAME` describes
/// it better than one reporting only a `SPDRP_DEVICEDESC` (typically the
/// generic "USB Composite Device"), which still beats falling back to the raw
/// VID/PID string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum NameSource {
    Placeholder,
    DeviceDesc,
    FriendlyName,
}

/// Folds one enumerated device node into the deduplicated device list.
///
/// Nodes belonging to the same physical device share a VID&PID, so the best
/// name and the most specific class are taken across all of them — independent
/// of the order the OS happens to enumerate them in.
///
/// Lives here rather than in the Windows-only backend because it contains no
/// FFI, and so stays testable on every platform.
pub fn merge(out: &mut Vec<(UsbDevice, NameSource)>, device: UsbDevice, source: NameSource) {
    let Some((existing, existing_source)) = out.iter_mut().find(|(d, _)| d.id == device.id) else {
        out.push((device, source));
        return;
    };

    if source > *existing_source {
        existing.name = device.name;
        *existing_source = source;
    }
    // Assigning Other over Other is a no-op, so this needs no second guard.
    if existing.class == DeviceClass::Other {
        existing.class = device.class;
    }
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
    /// A Win32 API call failed. Carries its own full message, since no tool is
    /// involved and the ControlMyMonitor wording would be misleading.
    #[error("{0}")]
    Win32(String),
    #[error("{0}")]
    Io(#[from] std::io::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(id: &str, name: &str, class: DeviceClass) -> UsbDevice {
        UsbDevice { id: id.into(), name: name.into(), class }
    }

    /// The composite-device case: the parent node enumerates first carrying the
    /// generic DEVICEDESC, and a child later reports the real FRIENDLYNAME.
    #[test]
    fn a_later_friendly_name_replaces_an_earlier_device_desc() {
        let mut out = Vec::new();
        merge(&mut out, dev("VID_046D&PID_C52B", "USB Composite Device", DeviceClass::Other), NameSource::DeviceDesc);
        merge(&mut out, dev("VID_046D&PID_C52B", "Logitech USB Receiver", DeviceClass::Mouse), NameSource::FriendlyName);

        assert_eq!(out.len(), 1, "nodes sharing a VID&PID must collapse to one device");
        assert_eq!(out[0].0.name, "Logitech USB Receiver");
        assert_eq!(out[0].0.class, DeviceClass::Mouse);
    }

    /// The same merge must hold with the nodes enumerated the other way round.
    #[test]
    fn an_earlier_friendly_name_survives_a_later_device_desc() {
        let mut out = Vec::new();
        merge(&mut out, dev("VID_046D&PID_C52B", "Logitech USB Receiver", DeviceClass::Mouse), NameSource::FriendlyName);
        merge(&mut out, dev("VID_046D&PID_C52B", "USB Composite Device", DeviceClass::Other), NameSource::DeviceDesc);

        assert_eq!(out.len(), 1);
        assert_eq!(out[0].0.name, "Logitech USB Receiver");
        assert_eq!(out[0].0.class, DeviceClass::Mouse);
    }

    #[test]
    fn a_real_name_replaces_the_vid_pid_placeholder() {
        let mut out = Vec::new();
        merge(&mut out, dev("VID_8087&PID_0032", "VID_8087&PID_0032", DeviceClass::Other), NameSource::Placeholder);
        merge(&mut out, dev("VID_8087&PID_0032", "Intel Wireless Bluetooth", DeviceClass::Other), NameSource::DeviceDesc);

        assert_eq!(out[0].0.name, "Intel Wireless Bluetooth");
    }

    #[test]
    fn the_placeholder_never_overwrites_a_real_name() {
        let mut out = Vec::new();
        merge(&mut out, dev("VID_8087&PID_0032", "Intel Wireless Bluetooth", DeviceClass::Other), NameSource::DeviceDesc);
        merge(&mut out, dev("VID_8087&PID_0032", "VID_8087&PID_0032", DeviceClass::Other), NameSource::Placeholder);

        assert_eq!(out[0].0.name, "Intel Wireless Bluetooth");
    }

    /// Equal rank keeps the incumbent, so enumeration order cannot make the
    /// list flicker between two equally-good names across refreshes.
    #[test]
    fn equal_rank_keeps_the_first_node_seen() {
        let mut out = Vec::new();
        merge(&mut out, dev("VID_1B1C&PID_1B7C", "First", DeviceClass::Other), NameSource::FriendlyName);
        merge(&mut out, dev("VID_1B1C&PID_1B7C", "Second", DeviceClass::Other), NameSource::FriendlyName);

        assert_eq!(out[0].0.name, "First");
    }

    /// Class and name are taken independently: a node can contribute the class
    /// without winning the name, and vice versa.
    #[test]
    fn a_specific_class_is_adopted_even_from_a_lower_ranked_node() {
        let mut out = Vec::new();
        merge(&mut out, dev("VID_1B1C&PID_1B7C", "Corsair K70", DeviceClass::Other), NameSource::FriendlyName);
        merge(&mut out, dev("VID_1B1C&PID_1B7C", "HID Keyboard Device", DeviceClass::Keyboard), NameSource::DeviceDesc);

        assert_eq!(out[0].0.name, "Corsair K70");
        assert_eq!(out[0].0.class, DeviceClass::Keyboard);
    }

    #[test]
    fn an_established_class_is_not_downgraded_to_other() {
        let mut out = Vec::new();
        merge(&mut out, dev("VID_046D&PID_C08B", "G502", DeviceClass::Mouse), NameSource::DeviceDesc);
        merge(&mut out, dev("VID_046D&PID_C08B", "G502 Gaming Mouse", DeviceClass::Other), NameSource::FriendlyName);

        assert_eq!(out[0].0.class, DeviceClass::Mouse);
        assert_eq!(out[0].0.name, "G502 Gaming Mouse");
    }

    #[test]
    fn distinct_vid_pid_pairs_stay_separate() {
        let mut out = Vec::new();
        merge(&mut out, dev("VID_046D&PID_C08B", "Mouse", DeviceClass::Mouse), NameSource::FriendlyName);
        merge(&mut out, dev("VID_046D&PID_085C", "Camera", DeviceClass::Camera), NameSource::FriendlyName);

        assert_eq!(out.len(), 2);
    }
}
