//! Sequence for Ambiq Apollo chips.

use std::{
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

use crate::{
    architecture::arm::{
        ArmError,
        core::armv7m::{Aircr, Demcr, Dhcsr},
        memory::ArmMemoryInterface,
        sequences::ArmDebugSequence,
    },
    core::memory_mapped_registers::MemoryMappedRegister,
    memory_mapped_bitfield_register,
};

#[derive(Copy, Clone, PartialEq, PartialOrd, Hash, Eq, Debug)]
enum SecBoot {
    Disabled = 0,
    Enabled = 1,
    Error = 2,
}

impl From<SecBoot> for u8 {
    fn from(value: SecBoot) -> Self {
        value as u8
    }
}

impl From<u8> for SecBoot {
    fn from(value: u8) -> Self {
        match value {
            0 => SecBoot::Disabled,
            1 => SecBoot::Enabled,
            2 => SecBoot::Error,
            _ => unreachable!(),
        }
    }
}

memory_mapped_bitfield_register! {
    /// BOOTLOADER Register
    ///
    /// Bootloader and secure boot functions.
    ///
    /// Reference:
    /// 3.9.2.17 of Apollo 3 Blue SoC Datasheet.
    pub struct Bootloader(u32);
    0x4002_01a0, "Bootloader",
    impl From;

    /// Determines whether bootloader code is visible at address
    /// 0x0000_0000 or not.
    ///
    /// Resets to 1, write 1 to clear.
    pub bootloader_low, set_bootloader_low : 0;

    /// Enables system visibility to bootloader until set.
    ///
    /// Write 1 to clear.
    pub sblock, set_sblock : 1;

    /// Enables writes to flash protection register set.
    ///
    /// Write 1 to clear.
    pub protlock, set_protlock : 2;

    /// Indicates whether secure boot feature is enabled.
    pub u8, into SecBoot, secbootfeature, _ : 27, 26;

    /// Indicates whether secure boot on cold reset is enabled.
    pub secboot, _ : 29, 28;

    /// Indicates whether secure boot on warm reset is enabled.
    pub secbootonrst, _ : 29, 28;
}

memory_mapped_bitfield_register! {
    /// Scratch0 register
    ///
    /// Reference:
    /// 3.9.2.19 of Apollo 3 Blue SoC Datasheet.
    pub struct Scratch0(u32);
    0x4002_01b0, "Scratch0",
    impl From;
}

/// Debug sequence for Apollo3 family MCUs.
#[derive(Debug)]
pub struct Apollo3 {}

impl Apollo3 {
    /// Create a sequence handle for Apollo3 MCUs.
    pub fn create() -> Arc<dyn ArmDebugSequence> {
        Arc::new(Self {})
    }
}

impl ArmDebugSequence for Apollo3 {
    fn reset_system(
        &self,
        probe: &mut dyn ArmMemoryInterface,
        _core_type: crate::CoreType,
        _: Option<u64>,
    ) -> Result<(), ArmError> {
        // Halt the core
        let mut dhcsr = Dhcsr(0);
        dhcsr.set_c_halt(true);
        dhcsr.set_c_debugen(true);
        dhcsr.enable_write();
        probe.write_word_32(Dhcsr::get_mmio_address(), dhcsr.into())?;
        probe.flush()?;

        let bootloader = Bootloader(probe.read_word_32(Bootloader::get_mmio_address())?);
        if bootloader.secbootfeature() == SecBoot::Enabled {
            // Set Scratch0 indicating that the bootloader needs to
            // run, then halt when it is finished.
            tracing::debug!(
                "Secure mode: bootloader needs to run and will halt when it has finished."
            );
            let scratch0 = Scratch0(probe.read_word_32(Scratch0::get_mmio_address())? | 1);
            probe.write_word_32(Scratch0::get_mmio_address(), scratch0.into())?;
            probe.flush()?;
        } else {
            // Clear VECTOR CATCH and set TRCENA
            let mut demcr: Demcr = probe.read_word_32(Demcr::get_mmio_address())?.into();
            demcr.set_trcena(true);
            demcr.set_vc_corereset(false);
            probe.write_word_32(Demcr::get_mmio_address(), demcr.into())?;
            probe.flush()?;
        }

        let mut aircr = Aircr(0);
        aircr.vectkey();
        aircr.set_sysresetreq(true);
        probe.write_word_32(Aircr::get_mmio_address(), aircr.into())?;
        probe.flush()?;
        tracing::trace!("Reset requested..");

        let start = Instant::now();
        while start.elapsed() < Duration::from_millis(600) {
            let dhcsr = Dhcsr(probe.read_word_32(Dhcsr::get_mmio_address())?);
            if dhcsr.s_reset_st() {
                tracing::trace!("System reset was successful");
                return Ok(());
            }

            thread::sleep(Duration::from_millis(100));
        }

        // Timed out, let's try one more time with a regular reset
        // sequence
        if bootloader.secbootfeature() == SecBoot::Enabled {
            // Should we make cortex_m_*() sequences pub(crate)?
            tracing::trace!("Secure part timed out. Reverting to standard reset sequence");
            let mut demcr: Demcr = probe.read_word_32(Demcr::get_mmio_address())?.into();
            demcr.set_vc_corereset(false);
            probe.write_word_32(Demcr::get_mmio_address(), demcr.into())?;
            probe.flush()?;

            aircr = Aircr(0);
            aircr.vectkey();
            aircr.set_sysresetreq(true);
            probe.write_word_32(Aircr::get_mmio_address(), aircr.into())?;
            probe.flush()?;

            let start = Instant::now();
            while start.elapsed() < Duration::from_millis(600) {
                let dhcsr = Dhcsr(probe.read_word_32(Dhcsr::get_mmio_address())?);
                if dhcsr.s_reset_st() {
                    tracing::trace!("System reset was successful");
                    return Ok(());
                }

                thread::sleep(Duration::from_millis(100));
            }
        }

        tracing::error!("System reset timed out");
        Err(ArmError::Timeout)
    }
}
