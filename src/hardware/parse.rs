/// One monitor block from a ControlMyMonitor `/stext` dump.
#[derive(Debug, Clone, PartialEq)]
pub struct StextMonitor {
    /// e.g. `\\.\DISPLAY1\Monitor0` — used only to correlate with GDI output.
    pub device_name: String,
    pub model: String,
    pub serial: String,
    /// Current VCP 60 value, if the dump contained a VCP 60 block.
    pub current_input: Option<u16>,
}

/// Parses "VID_046D&PID_085C" (either field order) into numeric ids.
pub fn parse_vid_pid(id: &str) -> Option<(u16, u16)> {
    let upper = id.to_ascii_uppercase();
    let mut vid = None;
    let mut pid = None;

    for part in upper.split(|c: char| c == '&' || c == '\\') {
        if let Some(hex) = part.strip_prefix("VID_") {
            vid = u16::from_str_radix(hex, 16).ok();
        } else if let Some(hex) = part.strip_prefix("PID_") {
            pid = u16::from_str_radix(hex, 16).ok();
        }
    }

    Some((vid?, pid?))
}

/// Canonical form: uppercase hex, zero-padded to four digits.
pub fn format_vid_pid(vid: u16, pid: u16) -> String {
    format!("VID_{vid:04X}&PID_{pid:04X}")
}

/// Pulls the canonical VID/PID pair out of a SetupAPI device instance id.
///
/// Instance ids look like `USB\VID_046D&PID_085C&MI_01\7&2A1B3C&0&0001`; the
/// trailing `&MI_xx` interface segment and the instance suffix are dropped, so
/// every node belonging to one physical device collapses to the same string.
pub fn extract_id_from_instance(instance_id: &str) -> Option<String> {
    let (vid, pid) = parse_vid_pid(instance_id)?;
    Some(format_vid_pid(vid, pid))
}

/// Parses a ControlMyMonitor `/stext` dump.
///
/// The dump repeats a block per monitor per VCP code. Blocks are accumulated
/// by device name, and only the VCP 60 block contributes `current_input`.
// ponytail: line-oriented "Key: Value" scan rather than a grammar; the format
// is stable and NirSoft-generated. Revisit only if /stext output changes shape.
pub fn parse_stext(text: &str) -> Vec<StextMonitor> {
    let mut out: Vec<StextMonitor> = Vec::new();
    let mut device_name = String::new();
    let mut model = String::new();
    let mut serial = String::new();
    let mut vcp_code: Option<u16> = None;
    let mut current: Option<u16> = None;

    let flush = |out: &mut Vec<StextMonitor>,
                 device_name: &str,
                 model: &str,
                 serial: &str,
                 vcp_code: Option<u16>,
                 current: Option<u16>| {
        if device_name.is_empty() {
            return;
        }
        let input = if vcp_code == Some(60) { current } else { None };

        if let Some(existing) = out.iter_mut().find(|m: &&mut StextMonitor| m.device_name == device_name) {
            if input.is_some() {
                existing.current_input = input;
            }
            return;
        }

        out.push(StextMonitor {
            device_name: device_name.to_string(),
            model: model.to_string(),
            serial: serial.to_string(),
            current_input: input,
        });
    };

    for line in text.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim();

        match key {
            "Monitor Device Name" => {
                flush(&mut out, &device_name, &model, &serial, vcp_code, current);
                device_name = value.to_string();
                model.clear();
                serial.clear();
                vcp_code = None;
                current = None;
            }
            "Monitor Name" => model = value.to_string(),
            "Serial Number" => serial = value.to_string(),
            "VCP Code" => vcp_code = value.parse().ok(),
            "Current Value" => current = value.parse().ok(),
            _ => {}
        }
    }
    flush(&mut out, &device_name, &model, &serial, vcp_code, current);

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_vid_pid_pairs() {
        assert_eq!(parse_vid_pid("VID_046D&PID_085C"), Some((0x046D, 0x085C)));
        assert_eq!(parse_vid_pid("PID_085C&VID_046D"), Some((0x046D, 0x085C)));
        assert_eq!(parse_vid_pid("garbage"), None);
        assert_eq!(parse_vid_pid("VID_ZZZZ&PID_085C"), None);
    }

    #[test]
    fn formats_ids_as_uppercase_padded_hex() {
        assert_eq!(format_vid_pid(0x046D, 0x085C), "VID_046D&PID_085C");
        assert_eq!(format_vid_pid(0x1, 0x2), "VID_0001&PID_0002");
    }

    #[test]
    fn extracts_id_from_a_device_instance_path() {
        // Real SetupAPI instance IDs carry a trailing instance segment.
        assert_eq!(
            extract_id_from_instance(r"USB\VID_046D&PID_085C&MI_01\7&2A1B3C&0&0001"),
            Some("VID_046D&PID_085C".to_string())
        );
        assert_eq!(extract_id_from_instance(r"ACPI\PNP0C0C\2&daba3ff&2"), None);
    }

    #[test]
    fn extraction_is_case_insensitive_but_output_is_uppercase() {
        assert_eq!(
            extract_id_from_instance(r"usb\vid_046d&pid_085c\5&12ab"),
            Some("VID_046D&PID_085C".to_string())
        );
    }

    const SAMPLE_STEXT: &str = "\
==================================================
Monitor Device Name: \\\\.\\DISPLAY1\\Monitor0
Monitor Name        : DELL U2720Q
Serial Number       : ABC123456
VCP Code            : 60
VCP Code Name       : Input Select
Current Value       : 15
Maximum Value       : 18
==================================================
Monitor Device Name: \\\\.\\DISPLAY2\\Monitor0
Monitor Name        : LG HDR 4K
Serial Number       : XYZ987654
VCP Code            : 60
VCP Code Name       : Input Select
Current Value       : 17
Maximum Value       : 18
==================================================
";

    #[test]
    fn parses_stext_into_monitors() {
        let monitors = parse_stext(SAMPLE_STEXT);
        assert_eq!(monitors.len(), 2);
        assert_eq!(monitors[0].device_name, r"\\.\DISPLAY1\Monitor0");
        assert_eq!(monitors[0].model, "DELL U2720Q");
        assert_eq!(monitors[0].serial, "ABC123456");
        assert_eq!(monitors[0].current_input, Some(15));
        assert_eq!(monitors[1].serial, "XYZ987654");
        assert_eq!(monitors[1].current_input, Some(17));
    }

    #[test]
    fn ignores_vcp_codes_other_than_sixty() {
        let text = "\
Monitor Device Name: \\\\.\\DISPLAY1\\Monitor0
Monitor Name        : DELL U2720Q
Serial Number       : ABC123456
VCP Code            : 16
VCP Code Name       : Brightness
Current Value       : 75
Maximum Value       : 100
";
        let monitors = parse_stext(text);
        assert_eq!(monitors.len(), 1);
        assert_eq!(monitors[0].current_input, None);
    }

    #[test]
    fn tolerates_empty_or_garbage_input() {
        assert!(parse_stext("").is_empty());
        assert!(parse_stext("no colons here at all").is_empty());
    }
}
