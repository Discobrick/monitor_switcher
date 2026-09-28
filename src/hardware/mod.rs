pub mod parse;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

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
pub use mock::{
    apply_input, list_devices, list_monitors, read_input, set_all_present, set_device_present,
    set_fail_next_command, set_tool_present,
};

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

/// One monitor's virtual-desktop rectangle, as reported by GDI.
///
/// Split out from the Win32 enumeration so the correlation with the
/// ControlMyMonitor dump — the part with actual logic in it — is testable on
/// every platform.
#[derive(Debug, Clone, PartialEq)]
pub struct MonitorGeometry {
    /// `\\.\DISPLAY1` — the adapter device, with no `\Monitor0` suffix.
    pub device: String,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub is_primary: bool,
}

/// `\\.\DISPLAY1\Monitor0` -> `\\.\DISPLAY1`, the form GDI reports.
///
/// Only a trailing `\MonitorN` is removed. Splitting on the last backslash
/// unconditionally would turn a dump that already names the adapter alone into
/// the bare `\\.` prefix, which then matches every monitor.
pub fn display_prefix(device_name: &str) -> &str {
    match device_name.rsplit_once('\\') {
        Some((head, tail)) if tail.get(..7).is_some_and(|p| p.eq_ignore_ascii_case("monitor")) => {
            head
        }
        _ => device_name,
    }
}

/// Merges ControlMyMonitor identity with GDI geometry.
///
/// Identity drives the result: a monitor DDC/CI cannot describe is one the app
/// cannot switch, so the dump is the source of truth for which monitors exist.
/// Geometry is joined on the `\\.\DISPLAYn` prefix that both sides report, and
/// a monitor GDI does not place falls back to a plausible rectangle rather than
/// disappearing from the list.
pub fn combine(stext: Vec<parse::StextMonitor>, gdi: &[MonitorGeometry]) -> Vec<MonitorInfo> {
    stext
        .into_iter()
        .map(|s| {
            let g = gdi
                .iter()
                .find(|g| g.device.eq_ignore_ascii_case(display_prefix(&s.device_name)));

            MonitorInfo {
                serial: s.serial,
                model: s.model,
                device_name: s.device_name,
                x: g.map_or(0, |g| g.x),
                y: g.map_or(0, |g| g.y),
                width: g.map_or(1920, |g| g.width),
                height: g.map_or(1080, |g| g.height),
                is_primary: g.is_some_and(|g| g.is_primary),
                current_input: s.current_input,
            }
        })
        .collect()
}

/// Turns a `/GetValue` exit code into a VCP 60 input.
///
/// Only the low byte is the input: some monitors (seen on a Dell) put junk in
/// the high byte, returning 0x0F0F for input 15. Zero is ControlMyMonitor's
/// failure code and is never a valid input.
pub fn input_from_exit_code(code: i32) -> Option<u16> {
    match (code & 0xFF) as u16 {
        0 => None,
        v => Some(v),
    }
}

/// The temp file a `/stext` dump is written to, removed on every exit path.
///
/// Lives here rather than in the Windows backend because none of it is FFI:
/// the naming and the cleanup are what make concurrent dumps safe, so they are
/// testable on any platform.
pub struct TempDump(PathBuf);

impl TempDump {
    pub fn new() -> Self {
        // The PID keeps concurrent processes apart; the counter keeps
        // concurrent calls *within* a process apart, which a tray app does as
        // soon as a background poll overlaps a user-triggered read.
        static SEQ: AtomicU64 = AtomicU64::new(0);
        Self::at(std::env::temp_dir().join(format!(
            "monsw_{}_{}.txt",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        )))
    }

    /// Takes ownership of `path`, clearing anything already there.
    ///
    /// A hard-killed run skips `Drop`. If its PID is later reused and the tool
    /// then exits 0 without writing, that leftover would be parsed as the
    /// current truth, so it is cleared before spawning rather than after.
    fn at(path: PathBuf) -> Self {
        let _ = std::fs::remove_file(&path);
        Self(path)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Default for TempDump {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for TempDump {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Decodes and parses a `/stext` dump.
///
/// A dump that has content but yields no monitors means the file was not the
/// text this parser expects — a BOM-less UTF-16 dump decoded lossily, say.
/// Returning `Ok(vec![])` there would render as "this machine has no DDC/CI
/// monitors", which is the one thing the user cannot tell apart from a real
/// empty result, so it is an error instead.
pub fn parse_dump(bytes: &[u8]) -> Result<Vec<parse::StextMonitor>, HardwareError> {
    let text = decode_dump(bytes);
    let monitors = parse::parse_smonitors(&text);

    if monitors.is_empty() && !text.trim().is_empty() {
        return Err(HardwareError::ToolFailed(format!(
            "wrote a {}-byte dump that contained no monitor entries",
            bytes.len()
        )));
    }

    Ok(monitors)
}

/// Decodes a `/stext` dump, which ControlMyMonitor writes as UTF-16 or ANSI
/// depending on how it was invoked. Reading the BOM covers both without a
/// command-line flag to force one.
pub fn decode_dump(bytes: &[u8]) -> String {
    match bytes {
        [0xFF, 0xFE, rest @ ..] => decode_utf16(rest, u16::from_le_bytes),
        [0xFE, 0xFF, rest @ ..] => decode_utf16(rest, u16::from_be_bytes),
        [0xEF, 0xBB, 0xBF, rest @ ..] => String::from_utf8_lossy(rest).into_owned(),
        _ => String::from_utf8_lossy(bytes).into_owned(),
    }
}

fn decode_utf16(bytes: &[u8], to_unit: fn([u8; 2]) -> u16) -> String {
    let units: Vec<u16> = bytes.chunks_exact(2).map(|c| to_unit([c[0], c[1]])).collect();
    String::from_utf16_lossy(&units)
}

/// Turns a non-zero ControlMyMonitor exit into an error.
///
/// The tool is silent on stderr for most failures, so the exit code is kept as
/// the fallback detail rather than reporting an empty message.
pub fn tool_error(code: Option<i32>, stderr: &[u8]) -> HardwareError {
    let stderr = String::from_utf8_lossy(stderr).trim().to_string();
    HardwareError::ToolFailed(if stderr.is_empty() {
        match code {
            Some(code) => format!("exit code {code}"),
            None => "terminated by a signal".to_string(),
        }
    } else {
        stderr
    })
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

    // -- monitor correlation ------------------------------------------------

    fn stext(device_name: &str, serial: &str, input: Option<u16>) -> parse::StextMonitor {
        parse::StextMonitor {
            device_name: device_name.into(),
            model: "DELL U2720Q".into(),
            serial: serial.into(),
            current_input: input,
        }
    }

    fn geom(device: &str, x: i32, y: i32, w: i32, h: i32, primary: bool) -> MonitorGeometry {
        MonitorGeometry { device: device.into(), x, y, width: w, height: h, is_primary: primary }
    }

    #[test]
    fn the_monitor_suffix_is_stripped_to_the_gdi_device() {
        assert_eq!(display_prefix(r"\\.\DISPLAY1\Monitor0"), r"\\.\DISPLAY1");
    }

    /// A dump that reports the adapter alone must not be truncated to `\\.`.
    #[test]
    fn a_name_with_no_monitor_suffix_is_left_alone() {
        assert_eq!(display_prefix(r"\\.\DISPLAY1"), r"\\.\DISPLAY1");
        assert_eq!(display_prefix("DISPLAY1"), "DISPLAY1");
    }

    #[test]
    fn geometry_is_joined_onto_identity_by_display_prefix() {
        let out = combine(
            vec![stext(r"\\.\DISPLAY2\Monitor0", "XYZ987654", Some(17))],
            &[
                geom(r"\\.\DISPLAY1", 0, 0, 3840, 2160, true),
                geom(r"\\.\DISPLAY2", 3840, -400, 1080, 1920, false),
            ],
        );

        assert_eq!(out.len(), 1);
        assert_eq!((out[0].x, out[0].y, out[0].width, out[0].height), (3840, -400, 1080, 1920));
        assert!(!out[0].is_primary);
        assert_eq!(out[0].serial, "XYZ987654");
        assert_eq!(out[0].current_input, Some(17));
    }

    /// GDI reports `\\.\DISPLAY1` while some dumps carry `\\.\Display1`.
    #[test]
    fn the_join_ignores_case() {
        let out = combine(
            vec![stext(r"\\.\Display1\Monitor0", "ABC123456", Some(15))],
            &[geom(r"\\.\DISPLAY1", 100, 200, 2560, 1440, true)],
        );

        assert_eq!((out[0].x, out[0].y, out[0].width, out[0].height), (100, 200, 2560, 1440));
        assert!(out[0].is_primary);
    }

    /// Identity, not geometry, decides what exists: an unmatched monitor is
    /// still switchable, so it keeps its place in the list.
    #[test]
    fn a_monitor_gdi_does_not_place_survives_with_a_fallback_rect() {
        let out = combine(
            vec![stext(r"\\.\DISPLAY9\Monitor0", "NOGDI0001", Some(18))],
            &[geom(r"\\.\DISPLAY1", 0, 0, 3840, 2160, true)],
        );

        assert_eq!(out.len(), 1);
        assert_eq!((out[0].x, out[0].y, out[0].width, out[0].height), (0, 0, 1920, 1080));
        assert!(!out[0].is_primary);
    }

    /// A prefix must not match a longer one that merely starts the same way.
    #[test]
    fn display1_does_not_borrow_display10s_geometry() {
        let out = combine(
            vec![stext(r"\\.\DISPLAY1\Monitor0", "ABC123456", None)],
            &[geom(r"\\.\DISPLAY10", 5000, 0, 1280, 720, false)],
        );

        assert_eq!((out[0].width, out[0].height), (1920, 1080));
    }

    #[test]
    fn combine_preserves_dump_order_for_every_monitor() {
        let out = combine(
            vec![
                stext(r"\\.\DISPLAY1\Monitor0", "AAA", Some(15)),
                stext(r"\\.\DISPLAY2\Monitor0", "BBB", Some(17)),
                stext(r"\\.\DISPLAY3\Monitor0", "CCC", None),
            ],
            &[geom(r"\\.\DISPLAY3", -1920, 300, 1920, 1080, false)],
        );

        let serials: Vec<&str> = out.iter().map(|m| m.serial.as_str()).collect();
        assert_eq!(serials, ["AAA", "BBB", "CCC"]);
        assert_eq!(out[2].x, -1920);
    }

    // -- /GetValue exit code -------------------------------------------------

    #[test]
    fn get_value_keeps_only_the_low_byte_and_treats_zero_as_failure() {
        assert_eq!(input_from_exit_code(17), Some(17));
        assert_eq!(input_from_exit_code(0x0F0F), Some(15), "Dell junk high byte");
        assert_eq!(input_from_exit_code(0), None);
    }

    // -- dump decoding ------------------------------------------------------

    #[test]
    fn a_utf16_le_dump_decodes_without_its_bom_or_nul_padding() {
        let mut bytes = vec![0xFF, 0xFE];
        bytes.extend("Monitor Name: DELL".encode_utf16().flat_map(u16::to_le_bytes));
        assert_eq!(decode_dump(&bytes), "Monitor Name: DELL");
    }

    #[test]
    fn a_utf16_be_dump_decodes_too() {
        let mut bytes = vec![0xFE, 0xFF];
        bytes.extend("Serial: ABC123".encode_utf16().flat_map(u16::to_be_bytes));
        assert_eq!(decode_dump(&bytes), "Serial: ABC123");
    }

    #[test]
    fn a_utf8_bom_is_stripped_so_the_first_key_still_parses() {
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice(b"Monitor Name: DELL");
        assert_eq!(decode_dump(&bytes), "Monitor Name: DELL");
    }

    #[test]
    fn a_plain_ansi_dump_is_passed_through() {
        assert_eq!(decode_dump(b"Monitor Name: DELL"), "Monitor Name: DELL");
    }

    #[test]
    fn an_empty_dump_decodes_to_an_empty_string() {
        assert_eq!(decode_dump(&[]), "");
        assert_eq!(decode_dump(&[0xFF, 0xFE]), "");
    }

    /// A decoded dump has to survive the real parser, not just compare equal.
    #[test]
    fn a_utf16_dump_round_trips_through_the_parser() {
        let mut bytes = vec![0xFF, 0xFE];
        bytes.extend(DUMP.encode_utf16().flat_map(u16::to_le_bytes));

        let parsed = parse::parse_smonitors(&decode_dump(&bytes));
        assert_eq!(parsed.len(), 1, "the decoded UTF-16 dump must parse");
        assert_eq!(parsed[0].serial, r"MONITOR\PHLC156\{4d36e96e-e325-11ce-bfc1-08002be10318}\0007");
    }

    // -- tool exit status ---------------------------------------------------

    #[test]
    fn stderr_becomes_the_failure_detail_when_the_tool_says_anything() {
        let err = tool_error(Some(1), b"  monitor not found\r\n");
        assert!(matches!(err, HardwareError::ToolFailed(d) if d == "monitor not found"));
    }

    #[test]
    fn a_silent_failure_falls_back_to_the_exit_code() {
        let err = tool_error(Some(3), b"   \n");
        assert!(matches!(err, HardwareError::ToolFailed(d) if d == "exit code 3"));
    }

    #[test]
    fn a_signal_kill_is_reported_rather_than_shown_as_no_code() {
        let err = tool_error(None, b"");
        assert!(matches!(err, HardwareError::ToolFailed(d) if d == "terminated by a signal"));
    }

    // -- dump parsing guard -------------------------------------------------

    const DUMP: &str = "Monitor Device Name: \"\\\\.\\DISPLAY2\\Monitor0\"\r\n\
                        Monitor Name: \"PHL 273V7\"\r\n\
                        Serial Number: \"UK02215042535\"\r\n\
                        Monitor ID: \"MONITOR\\PHLC156\\{4d36e96e-e325-11ce-bfc1-08002be10318}\\0007\"\r\n";

    #[test]
    fn a_good_dump_parses_to_its_monitors() {
        let parsed = parse_dump(DUMP.as_bytes()).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].model, "PHL 273V7");
    }

    /// The failure this guards: a BOM-less UTF-16 dump decodes lossily to text
    /// nothing can parse. Without the guard the UI shows an empty monitor list
    /// and no error, which the user cannot tell apart from a machine that
    /// genuinely has no DDC/CI monitors.
    #[test]
    fn a_dump_that_parses_to_nothing_is_an_error_not_an_empty_list() {
        let bomless_utf16: Vec<u8> = DUMP.encode_utf16().flat_map(u16::to_le_bytes).collect();

        let err = parse_dump(&bomless_utf16)
            .expect_err("an undecodable dump must not masquerade as zero monitors");
        assert!(matches!(err, HardwareError::ToolFailed(_)));
    }

    #[test]
    fn readable_but_unrecognised_content_is_also_an_error() {
        let err = parse_dump(b"<html><body>Access denied</body></html>").unwrap_err();
        assert!(matches!(err, HardwareError::ToolFailed(_)));
    }

    /// A machine with no DDC/CI monitors is a real, non-error outcome, so a
    /// dump with nothing in it must still succeed.
    #[test]
    fn a_genuinely_empty_dump_is_not_an_error() {
        assert_eq!(parse_dump(b"").unwrap().len(), 0);
        assert_eq!(parse_dump(b"\r\n   \r\n").unwrap().len(), 0);
        // A BOM with no body is empty content, not garbage.
        assert_eq!(parse_dump(&[0xFF, 0xFE]).unwrap().len(), 0);
    }

    // -- temp dump lifecycle ------------------------------------------------

    /// Two dumps alive at once must not share a path: a tray app polls in the
    /// background while the user triggers reads, and with a shared path the
    /// first one dropped deletes the file the second is about to read.
    #[test]
    fn concurrent_dumps_get_distinct_paths() {
        let (a, b) = (TempDump::new(), TempDump::new());
        assert_ne!(a.path(), b.path());
    }

    #[test]
    fn a_dump_path_is_removed_when_it_goes_out_of_scope() {
        let path = {
            let dump = TempDump::new();
            std::fs::write(dump.path(), b"x").unwrap();
            dump.path().to_path_buf()
        };
        assert!(!path.exists(), "the dump file outlived its TempDump");
    }

    /// A hard-killed prior run skips Drop. If the tool then exits 0 without
    /// writing, a leftover at the same path would be parsed as current truth.
    #[test]
    fn construction_clears_a_leftover_file_at_the_same_path() {
        let path = std::env::temp_dir().join("monsw_stale_test.txt");
        std::fs::write(&path, b"stale dump from a killed run").unwrap();

        let dump = TempDump::at(path.clone());
        assert!(!path.exists(), "a stale dump survived construction");
        assert_eq!(dump.path(), path);
    }
}
