//! TI In-Circuit Debug Interface implementation.

use nusb::DeviceInfo;

use super::{DebugProbe, DebugProbeError, DebugProbeInfo, DebugProbeSelector, ProbeFactory};

/// A factory for creating [`Icdi`] probes.
#[derive(Debug)]
pub struct IcdiFactory;

impl std::fmt::Display for IcdiFactory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("TI ICDI")
    }
}

impl ProbeFactory for IcdiFactory {
    fn open(&self, selector: &DebugProbeSelector) -> Result<Box<dyn DebugProbe>, DebugProbeError> {
        Err(())
    }

    fn list_probes(&self) -> Vec<DebugProbeInfo> {
        list_icdi_devices()
    }
}

/// Finds all CMSIS-DAP devices, either v1 (HID) or v2 (WinUSB Bulk).
///
/// This method uses nusb to read device strings, which might fail due
/// to permission or driver errors, so it falls back to listing only
/// HID devices if it does not find any suitable devices.
#[tracing::instrument(skip_all)]
pub fn list_icdi_devices() -> Vec<DebugProbeInfo> {
    tracing::debug!("Searching for ICDI probes using nusb");

    let mut probes = match nusb::list_devices() {
        Ok(devices) => devices
            .filter_map(|device| get_icdi_info(&device))
            .collect(),
        Err(e) => {
            tracing::warn!("error listing devices with nusb: {:?}", e);
            vec![]
        }
    };

    tracing::debug!("Found {} ICDI probes total", probes.len());
    probes
}

/// Checks if a given Device is an ICDI probe, returning Some(DebugProbeInfo) if so.
fn get_icdi_info(device: &DeviceInfo) -> Option<DebugProbeInfo> {
    // Open device handle and read basic information
    let vendor_id = device.vendor_id();
    let product_id = device.product_id();

    // ICDI probes use Luminary Micro Inc's Vendor ID
    let icdi = is_luminary(vendor_id) && is_icdi(product_id);

    if icdi {
        tracing::trace!(
            "{}: ICDI device with {} interfaces",
            prod_str,
            device.interfaces().count()
        );

        Some(DebugProbeInfo::new(
            prod_str.to_string(),
            device.vendor_id,
            device.product_id(),
            sn_str.map(Into::into),
            &CmsisDapFactory,
            hid_interface,
        ))
    } else {
        None
    }
}
