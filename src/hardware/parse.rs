/// One monitor block from a ControlMyMonitor `/smonitors` dump.
#[derive(Debug, Clone, PartialEq)]
pub struct StextMonitor {
    /// e.g. `\\.\DISPLAY1\Monitor0` — used only to correlate with GDI output.
    pub device_name: String,
    pub model: String,
    /// PnP Monitor ID (`MONITOR\DELA28A\{...}\0001`): the stable key
    /// ControlMyMonitor accepts. See `parse_smonitors`.
    pub monitor_id: String,
    /// Current VCP 60 value, read separately via `/GetValue`.
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

/// Parses a ControlMyMonitor `/smonitors` dump: one blank-line-separated
/// block per monitor, values in double quotes.
///
/// The stable key stored in `monitor_id` is the PnP `Monitor ID`, because
/// ControlMyMonitor does not accept serial numbers as a monitor argument and
/// many monitors report none. `current_input` is left empty; the dump carries
/// no VCP values, so the caller reads them with `/GetValue`.
// ponytail: line-oriented "Key: Value" scan rather than a grammar; the format
// is stable and NirSoft-generated.
pub fn parse_smonitors(text: &str) -> Vec<StextMonitor> {
    let mut out: Vec<StextMonitor> = Vec::new();

    for line in text.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim().trim_matches('"').to_string();

        match key.trim() {
            "Monitor Device Name" => out.push(StextMonitor {
                device_name: value,
                model: String::new(),
                monitor_id: String::new(),
                current_input: None,
            }),
            "Monitor Name" => {
                if let Some(m) = out.last_mut() {
                    m.model = value;
                }
            }
            "Monitor ID" => {
                if let Some(m) = out.last_mut() {
                    // `MONITOR\DELA28A\{...}\0001`: the second segment is the
                    // EDID vendor+product code, the best name a blank model has.
                    if m.model.is_empty() {
                        m.model = value.split('\\').nth(1).unwrap_or_default().to_string();
                    }
                    m.monitor_id = value;
                }
            }
            _ => {}
        }
    }

    // A block without a Monitor ID cannot be addressed, so it is not a monitor.
    out.retain(|m| !m.monitor_id.is_empty());
    out
}

/// Pulls the input-select (VCP 60) "Possible Values" out of a `/stext` dump.
///
/// These come from the monitor's DDC capabilities string, so they are the
/// inputs it actually has. Empty when the monitor doesn't report any.
pub fn parse_possible_inputs(text: &str) -> Vec<u16> {
    let mut in_vcp60 = false;
    for line in text.lines() {
        let Some((key, value)) = line.split_once(':') else { continue };
        match key.trim() {
            "VCP Code" => in_vcp60 = value.trim() == "60",
            "Possible Values" if in_vcp60 => {
                return value.split(',').filter_map(|v| v.trim().parse().ok()).collect();
            }
            _ => {}
        }
    }
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Excerpt of a real `/stext` dump (Philips 273V7: VGA, DVI, HDMI).
    const STEXT: &str = "\
==================================================
VCP Code          : 10
VCP Code Name     : Brightness
Current Value     : 65
Possible Values   :
==================================================
VCP Code          : 60
VCP Code Name     : Input Select
Read-Write        : Read+Write
Current Value     : 17
Maximum Value     : 17
Possible Values   : 1, 3, 17
==================================================
VCP Code          : 62
Possible Values   : 5, 6
";

    #[test]
    fn possible_inputs_come_from_the_vcp60_block_only() {
        assert_eq!(parse_possible_inputs(STEXT), [1, 3, 17]);
        assert!(parse_possible_inputs("VCP Code : 60\nPossible Values : \n").is_empty());
        assert!(parse_possible_inputs("").is_empty());
    }

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

    /// Captured from a real machine: the first monitor reports no name and no
    /// serial, which is common and must still yield an addressable monitor.
    const SAMPLE: &str = r#"Monitor Device Name: "\\.\DISPLAY1\Monitor0"
Monitor Name: ""
Serial Number: ""
Adapter Name: "NVIDIA GeForce RTX 3080 Ti"
Monitor ID: "MONITOR\DELA28A\{4d36e96e-e325-11ce-bfc1-08002be10318}\0001"

Monitor Device Name: "\\.\DISPLAY2\Monitor0"
Monitor Name: "PHL 273V7"
Serial Number: "UK02215042535"
Adapter Name: "NVIDIA GeForce RTX 3080 Ti"
Monitor ID: "MONITOR\PHLC156\{4d36e96e-e325-11ce-bfc1-08002be10318}\0007"
"#;

    #[test]
    fn parses_smonitors_keyed_by_monitor_id() {
        let monitors = parse_smonitors(SAMPLE);
        assert_eq!(monitors.len(), 2);
        assert_eq!(monitors[0].device_name, r"\\.\DISPLAY1\Monitor0");
        assert_eq!(monitors[0].monitor_id, r"MONITOR\DELA28A\{4d36e96e-e325-11ce-bfc1-08002be10318}\0001");
        assert_eq!(monitors[0].model, "DELA28A", "a blank name falls back to the EDID code");
        assert_eq!(monitors[1].model, "PHL 273V7");
        assert_eq!(monitors[1].monitor_id, r"MONITOR\PHLC156\{4d36e96e-e325-11ce-bfc1-08002be10318}\0007");
        assert!(monitors.iter().all(|m| m.current_input.is_none()));
    }

    #[test]
    fn tolerates_empty_or_garbage_input() {
        assert!(parse_smonitors("").is_empty());
        assert!(parse_smonitors("no colons here at all").is_empty());
        assert!(parse_smonitors("Monitor Device Name: \"x\"").is_empty(), "no Monitor ID, not addressable");
    }
}
