//! USB enumeration via SetupAPI.
//!
//! SetupAPI is used rather than hidapi because hidapi cannot see non-HID
//! devices (webcams, hubs) and reports generic product strings. SetupAPI
//! exposes the same friendly names Device Manager shows.

use windows::core::PCWSTR;
use windows::Win32::Devices::DeviceAndDriverInstallation::{
    SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInfo, SetupDiGetClassDevsW,
    SetupDiGetDeviceInstanceIdW, SetupDiGetDeviceRegistryPropertyW, DIGCF_ALLCLASSES,
    DIGCF_PRESENT, HDEVINFO, SPDRP_CLASS, SPDRP_DEVICEDESC, SPDRP_FRIENDLYNAME, SP_DEVINFO_DATA,
};

use super::parse::extract_id_from_instance;
use super::{merge, DeviceClass, HardwareError, NameSource, UsbDevice};

/// Owns the device info set so it is released on every exit path, including
/// early returns and panics.
struct DevInfoSet(HDEVINFO);

impl Drop for DevInfoSet {
    fn drop(&mut self) {
        // SAFETY: `self.0` is a valid handle from SetupDiGetClassDevsW, owned
        // solely by this value and not used after drop.
        unsafe {
            let _ = SetupDiDestroyDeviceInfoList(self.0);
        }
    }
}

/// Enumerates every present device, keeping those carrying a VID/PID pair.
///
/// One physical device produces several device nodes (composite interfaces,
/// child HID collections), so results are deduplicated by VID&PID by
/// `super::merge`, which resolves the name and class across those nodes.
pub fn list_devices() -> Result<Vec<UsbDevice>, HardwareError> {
    let mut merged: Vec<(UsbDevice, NameSource)> = Vec::new();

    // SAFETY: SetupDiGetClassDevsW with a null class GUID and DIGCF_ALLCLASSES
    // returns a handle to all present devices, released by `DevInfoSet::drop`.
    let devinfo = DevInfoSet(unsafe {
        SetupDiGetClassDevsW(None, PCWSTR::null(), None, DIGCF_PRESENT | DIGCF_ALLCLASSES)
            .map_err(|e| HardwareError::Win32(format!("SetupDiGetClassDevs failed: {e}")))?
    });

    for index in 0.. {
        let mut data = SP_DEVINFO_DATA {
            cbSize: std::mem::size_of::<SP_DEVINFO_DATA>() as u32,
            ..Default::default()
        };

        // SAFETY: `data.cbSize` is set as the API requires; enumeration stops
        // when this returns an error (ERROR_NO_MORE_ITEMS).
        if unsafe { SetupDiEnumDeviceInfo(devinfo.0, index, &mut data) }.is_err() {
            break;
        }

        let Some(instance_id) = instance_id(devinfo.0, &data) else {
            continue;
        };
        let Some(id) = extract_id_from_instance(&instance_id) else {
            continue;
        };

        // The provenance is carried into the merge so that a friendly name from
        // any node outranks a description from any other, whatever the order.
        let (name, source) = registry_string(devinfo.0, &data, SPDRP_FRIENDLYNAME)
            .map(|n| (n, NameSource::FriendlyName))
            .or_else(|| {
                registry_string(devinfo.0, &data, SPDRP_DEVICEDESC)
                    .map(|n| (n, NameSource::DeviceDesc))
            })
            .unwrap_or_else(|| (id.clone(), NameSource::Placeholder));

        let class = registry_string(devinfo.0, &data, SPDRP_CLASS)
            .map(|c| DeviceClass::from_setup_class(&c))
            .unwrap_or(DeviceClass::Other);

        merge(&mut merged, UsbDevice { id, name, class }, source);
    }

    let mut out: Vec<UsbDevice> = merged.into_iter().map(|(d, _)| d).collect();
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    Ok(out)
}

fn instance_id(devinfo: HDEVINFO, data: &SP_DEVINFO_DATA) -> Option<String> {
    // ponytail: fixed buffer, no resize path — MAX_DEVICE_ID_LEN is 200 and
    // this holds 512 UTF-16 units.
    let mut buf = [0u16; 512];

    // SAFETY: the buffer length is passed as its true element count; the call
    // writes at most that many UTF-16 code units.
    unsafe { SetupDiGetDeviceInstanceIdW(devinfo, data, Some(&mut buf), None) }.ok()?;
    Some(from_wide(&buf))
}

fn registry_string(devinfo: HDEVINFO, data: &SP_DEVINFO_DATA, property: u32) -> Option<String> {
    let mut buf = vec![0u8; 512];
    let mut needed = 0u32;

    // SAFETY: the byte buffer length is passed accurately. REG_SZ properties
    // are returned as UTF-16, which is decoded below.
    let call = |buf: &mut [u8], needed: &mut u32| unsafe {
        SetupDiGetDeviceRegistryPropertyW(devinfo, data, property, None, Some(buf), Some(needed))
    };

    if call(&mut buf, &mut needed).is_err() {
        // Either the property is absent, or the buffer was too small and
        // `needed` now holds the real size — retry once at that size.
        if needed as usize <= buf.len() {
            return None;
        }
        buf = vec![0u8; needed as usize];
        call(&mut buf, &mut needed).ok()?;
    }

    let wide: Vec<u16> = buf
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();

    let s = from_wide(&wide);
    if s.is_empty() { None } else { Some(s) }
}

fn from_wide(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}
