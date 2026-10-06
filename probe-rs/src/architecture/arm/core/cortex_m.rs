//! Common functions and data types for Cortex-M core variants

use super::{CortexMState, Dfsr, registers::cortex_m::XPSR, update_core_status};
use crate::memory::{CoreMemoryInterface, Operation, OperationKind};
use crate::{
    Architecture, BreakpointCause, CoreInformation, CoreInterface, CoreRegister, CoreStatus,
    CoreType, Error, HaltReason, InstructionSet, MemoryInterface, MemoryMappedRegister,
    architecture::arm::{
        ArmError,
        core::registers::cortex_m::{FP, PC, RA, SP},
        memory::ArmMemoryInterface,
        sequences::ArmDebugSequence,
    },
    core::{CoreRegisters, RegisterId, RegisterValue, VectorCatchCondition},
    memory_mapped_bitfield_register,
    semihosting::SemihostingCommand,
    semihosting::decode_semihosting_syscall,
};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

memory_mapped_bitfield_register! {
    pub struct Vtor(u32);
    0xE000_ED08, "VTOR",
    impl From;
    /// This fields holds bits `[31:7]` of the table offset.
    pub tbloff, set_tbloff: 31, 7;
}

memory_mapped_bitfield_register! {
    pub struct Dhcsr(u32);
    0xE000_EDF0, "DHCSR",
    impl From;
    pub s_reset_st, _: 25;
    pub s_retire_st, _: 24;
    pub s_lockup, _: 19;
    pub s_sleep, _: 18;
    pub s_halt, _: 17;
    pub s_regrdy, _: 16;
    pub c_maskints, set_c_maskints: 3;
    pub c_step, set_c_step: 2;
    pub c_halt, set_c_halt: 1;
    pub c_debugen, set_c_debugen: 0;
}

impl Dhcsr {
    /// This function sets the bit to enable writes to this register.
    ///
    /// C1.6.3 Debug Halting Control and Status Register, DHCSR:
    /// Debug key:
    /// Software must write 0xA05F to this field to enable write accesses to bits
    /// `[15:0]`, otherwise the processor ignores the write access.
    pub fn enable_write(&mut self) {
        self.0 &= !(0xffff << 16);
        self.0 |= 0xa05f << 16;
    }
}

memory_mapped_bitfield_register! {
    /// Debug Exception and Monitor Control Register, DEMCR
    ///
    /// Only the vector catch bits common to ARMv6-M and ARMv7-M are modelled here.
    pub struct Demcr(u32);
    0xE000_EDFC, "DEMCR",
    impl From;
    pub vc_harderr, set_vc_harderr: 10;
    pub vc_corereset, set_vc_corereset: 0;
}

memory_mapped_bitfield_register! {
    pub struct Dcrsr(u32);
    0xE000_EDF4, "DCRSR",
    impl From;
    pub _, set_regwnr: 16;
    // If the processor does not implement the FP extension the REGSEL field is bits `[4:0]`, and bits `[6:5]` are Reserved, SBZ.
    // Increased to 7 bits on v8-M
    pub _, set_regsel: 7,0;
}

memory_mapped_bitfield_register! {
    pub struct Dcrdr(u32);
    0xE000_EDF8, "DCRDR",
    impl From;
}

memory_mapped_bitfield_register! {
    ///  Coprocessor Access Control Register
    pub struct Cpacr(u32);
    0xE000_ED88, "CPACR",
    impl From;
    pub fpu_privilege, _: 21,20;
}

impl Cpacr {
    pub fn fpu_present(&self) -> bool {
        self.fpu_privilege() != 0
    }
}

memory_mapped_bitfield_register! {
    ///  Media and VFP Feature Register 0
    pub struct Mvfr0(u32);
    0xE000_EF40, "MVFR0",
    impl From;
    pub fpdp, _: 11, 8;
    pub fpsp, _: 7, 4;
}

impl Mvfr0 {
    pub fn fp_present(&self) -> bool {
        self.fpdp() != 0 || self.fpsp() != 0
    }
}

pub enum SecurityExtension {
    NotImplemented,
    Implemented,
    ImplementedWithStateHandling,
    Reserved,
}

impl From<u8> for SecurityExtension {
    fn from(value: u8) -> Self {
        match value {
            0b0000 => SecurityExtension::NotImplemented,
            0b0001 => SecurityExtension::Implemented,
            0b0011 => SecurityExtension::ImplementedWithStateHandling,
            _ => SecurityExtension::Reserved,
        }
    }
}

memory_mapped_bitfield_register! {
    /// Processor Feature Register 1
    pub struct IdPfr1(u32);
    0xE000_ED44, "ID_PFR1",
    impl From;
    /// Identifies support for the M-Profile programmer's model
    pub u8, m_prog_mod, _: 11, 8;
    /// Identifies whether the Security Extension is implemented
    pub u8, security, _: 7, 4;
}

impl IdPfr1 {
    pub fn security_present(&self) -> bool {
        matches!(
            self.security().into(),
            SecurityExtension::Implemented | SecurityExtension::ImplementedWithStateHandling
        )
    }
}

pub(crate) fn read_core_reg(
    memory: &mut dyn ArmMemoryInterface,
    addr: RegisterId,
) -> Result<u32, ArmError> {
    let mut dcrsr_val = Dcrsr(0);
    dcrsr_val.set_regwnr(false); // Perform a read.
    dcrsr_val.set_regsel(addr.into()); // The address of the register to read.

    // Select the register, then read the ready flag and the value. The flag is read first, so a
    // flag that comes back set means the value behind it had already settled.
    let mut ready = 0u32;
    let mut value = 0u32;
    memory.execute_operations(&mut [
        Operation::new(
            Dcrsr::get_mmio_address(),
            OperationKind::WriteWord32(dcrsr_val.into()),
        ),
        Operation::new(
            Dhcsr::get_mmio_address(),
            OperationKind::Read32(std::slice::from_mut(&mut ready)),
        ),
        Operation::new(
            Dcrdr::get_mmio_address(),
            OperationKind::Read32(std::slice::from_mut(&mut value)),
        ),
    ])?;

    if Dhcsr(ready).s_regrdy() {
        return Ok(value);
    }

    // Asked too early, which the flag is there to catch. Wait for it and read the value again.
    wait_for_core_register_transfer(memory, Duration::from_millis(100))?;

    memory.read_word_32(Dcrdr::get_mmio_address())
}

pub(crate) fn write_core_reg(
    memory: &mut dyn ArmMemoryInterface,
    addr: RegisterId,
    value: u32,
) -> Result<(), ArmError> {
    // write the DCRSR value to select the register we want to write.
    let mut dcrsr_val = Dcrsr(0);
    dcrsr_val.set_regwnr(true); // Perform a write.
    dcrsr_val.set_regsel(addr.into()); // The address of the register to write.

    // The DCRSR write clears the ready flag, so a flag that comes back set
    // means the transfer is done.
    let mut ready = 0u32;
    memory.execute_operations(&mut [
        Operation::new(Dcrdr::get_mmio_address(), OperationKind::WriteWord32(value)),
        Operation::new(
            Dcrsr::get_mmio_address(),
            OperationKind::WriteWord32(dcrsr_val.into()),
        ),
        Operation::new(
            Dhcsr::get_mmio_address(),
            OperationKind::Read32(std::slice::from_mut(&mut ready)),
        ),
    ])?;

    if Dhcsr(ready).s_regrdy() {
        return Ok(());
    }

    wait_for_core_register_transfer(memory, Duration::from_millis(100))
}

/// Leave Debug state, either running freely or stepping one instruction.
///
/// A single DHCSR write cannot clear C_HALT and change C_MASKINTS, so C_MASKINTS is settled in
/// its own write first. See ARM DDI 0419E C1.6.3, DDI 0403E.e C1.6.3 and DDI 0553B.v D1.2.38.
pub(crate) fn exit_halt(memory: &mut dyn ArmMemoryInterface, step: bool) -> Result<(), ArmError> {
    let mut dhcsr = Dhcsr(memory.read_word_32(Dhcsr::get_mmio_address())?);

    if !dhcsr.c_debugen() {
        tracing::warn!("Leaving halt while DHCSR.C_DEBUGEN is false");
    }

    // C_HALT stays 1 in this write, which is what makes the C_MASKINTS change predictable.
    if dhcsr.c_maskints() != step {
        dhcsr.set_c_maskints(step);
        dhcsr.enable_write();
        memory.write_word_32(Dhcsr::get_mmio_address(), dhcsr.into())?;
        memory.flush()?;
    }

    dhcsr.set_c_step(step);
    dhcsr.set_c_halt(false);
    dhcsr.set_c_debugen(true);
    dhcsr.enable_write();
    memory.write_word_32(Dhcsr::get_mmio_address(), dhcsr.into())?;
    memory.flush()
}

/// Set DHCSR.C_DEBUGEN, keeping the debug key so the write is not ignored.
pub(crate) fn enable_halting_debug(memory: &mut dyn ArmMemoryInterface) -> Result<(), ArmError> {
    let mut dhcsr = Dhcsr(memory.read_word_32(Dhcsr::get_mmio_address())?);
    dhcsr.set_c_debugen(true);
    dhcsr.enable_write();
    memory.write_word_32(Dhcsr::get_mmio_address(), dhcsr.into())
}

/// Enable or disable a vector catch condition.
///
/// ARMv6-M and ARMv7-M only. ARMv8-M also has VC_SFERR and keeps its own copy.
pub(crate) fn set_vector_catch(
    memory: &mut dyn ArmMemoryInterface,
    condition: VectorCatchCondition,
    enable: bool,
) -> Result<(), Error> {
    let mut demcr = Demcr(memory.read_word_32(Demcr::get_mmio_address())?);
    match condition {
        VectorCatchCondition::HardFault => demcr.set_vc_harderr(enable),
        VectorCatchCondition::CoreReset => demcr.set_vc_corereset(enable),
        VectorCatchCondition::All => {
            demcr.set_vc_harderr(enable);
            demcr.set_vc_corereset(enable);
        }
        VectorCatchCondition::SecureFault => {
            return Err(Error::Arm(ArmError::ArchitectureRequired(&["ARMv8"])));
        }
        VectorCatchCondition::Svc | VectorCatchCondition::Hlt => {
            return Err(Error::NotImplemented("vector catch condition Svc/Hlt"));
        }
    };

    memory.write_word_32(Demcr::get_mmio_address(), demcr.into())?;
    Ok(())
}

/// Check if the current breakpoint is a semihosting call.
///
/// Call this if you get some kind of breakpoint. Works on ARMv6-M, ARMv7-M and ARMv8-M.
pub(crate) fn check_for_semihosting(
    cached_command: Option<SemihostingCommand>,
    core: &mut dyn CoreInterface,
) -> Result<Option<SemihostingCommand>, Error> {
    // The Arm Semihosting Specification, specifies that the instruction
    // "BKPT 0xAB" (encoded as 0xBEAB) triggers a semihosting call.
    // <https://github.com/ARM-software/abi-aa/blob/main/semihosting/semihosting.rst#the-semihosting-interface>
    const TRAP_INSTRUCTION: [u8; 2] = [
        // instruction encoded as little endian
        0xAB, 0xBE,
    ];

    // We only want to decode the semihosting command once, since answering it might change some of the registers
    if let Some(command) = cached_command {
        return Ok(Some(command));
    }

    let pc: u32 = core.read_core_reg(core.program_counter().id)?.try_into()?;

    let mut actual_instruction = [0u8; 2];
    core.read_8(pc as u64, &mut actual_instruction)?;
    let actual_instruction = actual_instruction.as_slice();

    tracing::debug!(
        "Semihosting check pc={pc:#x} instruction={0:#02x}{1:#02x}",
        actual_instruction[1],
        actual_instruction[0]
    );

    let command = if TRAP_INSTRUCTION == actual_instruction {
        Some(decode_semihosting_syscall(core)?)
    } else {
        None
    };

    Ok(command)
}

fn wait_for_core_register_transfer(
    memory: &mut dyn ArmMemoryInterface,
    timeout: Duration,
) -> Result<(), ArmError> {
    // now we have to poll the dhcsr register, until the dhcsr.s_regrdy bit is set
    // (see C1-292, cortex m0 arm)
    let start = Instant::now();

    while start.elapsed() < timeout {
        let dhcsr_val = Dhcsr(memory.read_word_32(Dhcsr::get_mmio_address())?);

        if dhcsr_val.s_regrdy() {
            return Ok(());
        }
    }
    Err(ArmError::Timeout)
}

/// What differs between the Cortex-M architecture variants.
///
/// An implementor is the per-architecture payload [`CortexM`] carries: a unit
/// struct where there is nothing to remember, a value where there is. The
/// breakpoint functions take the memory interface rather than `&self` because
/// the FPB is reached only through memory.
pub trait CortexMVariant: Sized {
    /// Floating point registers the core reports, whether or not an FPU is
    /// fitted. ARMv6-M reports none.
    const FP_REGISTER_COUNT: usize;

    /// Per-attach probing. Runs on every [`CortexM::new`].
    fn new(memory: &mut dyn ArmMemoryInterface) -> Result<Self, Error>;

    /// Record FPU presence in `state`. Runs once, while `state` is uninitialized.
    fn detect_fpu(
        _memory: &mut dyn ArmMemoryInterface,
        _state: &mut CortexMState,
    ) -> Result<(), Error> {
        Ok(())
    }

    /// The register set this core exposes.
    fn registers(&self, state: &CortexMState) -> &'static CoreRegisters;

    /// Number of breakpoint comparators the FPB implements.
    fn available_breakpoint_units(memory: &mut dyn ArmMemoryInterface) -> Result<u32, Error>;

    /// The address in each comparator, or `None` where the comparator is disabled.
    fn hw_breakpoints(memory: &mut dyn ArmMemoryInterface) -> Result<Vec<Option<u64>>, Error>;

    /// Point comparator `index` at `addr`.
    fn set_hw_breakpoint(
        memory: &mut dyn ArmMemoryInterface,
        index: usize,
        addr: u64,
    ) -> Result<(), Error>;

    /// Disable comparator `index`.
    fn clear_hw_breakpoint(memory: &mut dyn ArmMemoryInterface, index: usize) -> Result<(), Error>;

    /// Global FPB enable.
    fn enable_breakpoints(memory: &mut dyn ArmMemoryInterface, enabled: bool) -> Result<(), Error>;

    /// ARMv6-M and ARMv7-M share this. ARMv8-M overrides it to reach VC_SFERR.
    fn enable_vector_catch(
        &self,
        memory: &mut dyn ArmMemoryInterface,
        condition: VectorCatchCondition,
    ) -> Result<(), Error> {
        enable_halting_debug(memory)?;
        set_vector_catch(memory, condition, true)
    }

    /// ARMv6-M and ARMv7-M share this. ARMv8-M overrides it to reach VC_SFERR.
    fn disable_vector_catch(
        &self,
        memory: &mut dyn ArmMemoryInterface,
        condition: VectorCatchCondition,
    ) -> Result<(), Error> {
        set_vector_catch(memory, condition, false)
    }
}

/// A Cortex-M core. Everything that varies between ARMv6-M, ARMv7-M and
/// ARMv8-M is reached through `V`.
pub struct CortexM<'probe, V: CortexMVariant> {
    memory: Box<dyn ArmMemoryInterface + 'probe>,

    state: &'probe mut CortexMState,

    sequence: Arc<dyn ArmDebugSequence>,

    /// Supplied by the caller rather than fixed by `V`, because one variant can
    /// serve several core types: ARMv7-M also drives Armv7em.
    core_type: CoreType,

    variant: V,
}

impl<'probe, V: CortexMVariant> CortexM<'probe, V> {
    pub(crate) fn new(
        mut memory: Box<dyn ArmMemoryInterface + 'probe>,
        state: &'probe mut CortexMState,
        sequence: Arc<dyn ArmDebugSequence>,
        core_type: CoreType,
    ) -> Result<Self, Error> {
        if !state.initialized() {
            // determine current state
            let dhcsr = Dhcsr(memory.read_word_32(Dhcsr::get_mmio_address())?);

            tracing::debug!("State when connecting: {:x?}", dhcsr);

            let core_state = if dhcsr.s_sleep() {
                CoreStatus::Sleeping
            } else if dhcsr.s_halt() {
                let dfsr = Dfsr(memory.read_word_32(Dfsr::get_mmio_address())?);

                let reason = dfsr.halt_reason();

                tracing::debug!("Core was halted when connecting, reason: {:?}", reason);

                CoreStatus::Halted(reason)
            } else {
                CoreStatus::Running
            };

            // Clear DFSR register. The bits in the register are sticky,
            // so we clear them here to ensure that that none are set.
            let dfsr_clear = Dfsr::clear_all();

            memory.write_word_32(Dfsr::get_mmio_address(), dfsr_clear.into())?;

            state.current_state = core_state;
            V::detect_fpu(&mut *memory, state)?;

            state.initialize();
        }

        let variant = V::new(&mut *memory)?;

        Ok(Self {
            memory,
            state,
            sequence,
            core_type,
            variant,
        })
    }

    fn set_core_status(&mut self, new_status: CoreStatus) {
        update_core_status(&mut self.memory, &mut self.state.current_state, new_status);
    }

    fn wait_for_status(
        &mut self,
        timeout: Duration,
        predicate: impl Fn(CoreStatus) -> bool,
    ) -> Result<(), Error> {
        let start = Instant::now();

        while !predicate(self.status()?) {
            if start.elapsed() >= timeout {
                return Err(Error::Arm(ArmError::Timeout));
            }
            // Wait a bit before polling again.
            std::thread::sleep(Duration::from_millis(1));
        }

        Ok(())
    }
}

impl<V: CortexMVariant> CoreInterface for CortexM<'_, V> {
    fn wait_for_core_halted(&mut self, timeout: Duration) -> Result<(), Error> {
        // Wait until halted state is active again.
        self.wait_for_status(timeout, |s| s.is_halted())
    }

    fn core_halted(&mut self) -> Result<bool, Error> {
        Ok(self.status()?.is_halted())
    }

    fn status(&mut self) -> Result<CoreStatus, Error> {
        let dhcsr = Dhcsr(self.memory.read_word_32(Dhcsr::get_mmio_address())?);

        if dhcsr.s_lockup() {
            tracing::debug!(
                "The core is in locked up status as a result of an unrecoverable exception"
            );

            self.state.clear_pending_step();
            self.set_core_status(CoreStatus::LockedUp);

            return Ok(CoreStatus::LockedUp);
        }

        if dhcsr.s_sleep() {
            // Check if we assumed the core to be halted
            if self.state.current_state.is_halted() {
                tracing::warn!("Expected core to be halted, but core is running");
            }

            self.set_core_status(CoreStatus::Sleeping);

            return Ok(CoreStatus::Sleeping);
        }

        // TODO: Handle lockup

        if dhcsr.s_halt() {
            let dfsr = Dfsr(self.memory.read_word_32(Dfsr::get_mmio_address())?);

            let mut reason = dfsr.halt_reason();
            reason = self.state.resolve_halt_reason(reason);

            // Clear bits from Dfsr register
            self.memory
                .write_word_32(Dfsr::get_mmio_address(), Dfsr::clear_all().into())?;

            // If the core was halted before, we cannot read the halt reason from the chip,
            // because we clear it directly after reading.
            if self.state.current_state.is_halted() {
                // There shouldn't be any bits set, otherwise it means
                // that the reason for the halt has changed. No bits set
                // means that we have an unknown HaltReason.
                if reason == HaltReason::Unknown {
                    tracing::debug!("Cached halt reason: {:?}", self.state.current_state);
                    return Ok(self.state.current_state);
                }

                tracing::debug!(
                    "Reason for halt has changed, old reason was {:?}, new reason is {:?}",
                    &self.state.current_state,
                    &reason
                );
            }

            // Set the status so any semihosting operations will know we're halted
            self.set_core_status(CoreStatus::Halted(reason));

            if let HaltReason::Breakpoint(_) = reason {
                self.state.semihosting_command =
                    check_for_semihosting(self.state.semihosting_command.take(), self)?;
                if let Some(command) = self.state.semihosting_command {
                    reason = HaltReason::Breakpoint(BreakpointCause::Semihosting(command));
                }

                // Set it again if it's changed
                self.set_core_status(CoreStatus::Halted(reason));
            }

            return Ok(CoreStatus::Halted(reason));
        }

        // Core is neither halted nor sleeping, so we assume it is running.
        if self.state.current_state.is_halted() {
            tracing::warn!("Core is running, but we expected it to be halted");
        }

        self.set_core_status(CoreStatus::Running);

        Ok(CoreStatus::Running)
    }

    fn halt(&mut self, timeout: Duration) -> Result<CoreInformation, Error> {
        // TODO: Generic halt support
        self.state.clear_pending_step();
        self.state.pc_written = false;

        let mut value = Dhcsr(0);
        value.set_c_halt(true);
        value.set_c_debugen(true);
        value.enable_write();

        self.memory
            .write_word_32(Dhcsr::get_mmio_address(), value.into())?;

        self.wait_for_core_halted(timeout)?;

        // try to read the program counter
        let pc_value = self.read_core_reg(self.program_counter().into())?;

        // get pc
        Ok(CoreInformation {
            pc: pc_value.try_into()?,
        })
    }

    fn run(&mut self) -> Result<(), Error> {
        // Before we run, we always perform a single instruction step, to account for possible
        // breakpoints that might get us stuck on the current instruction. If the PC was written
        // since we halted, the core is no longer on that instruction and there is nothing to
        // step over.
        if !self.state.pc_written {
            self.step()?;
            self.state.clear_pending_step();
        }
        self.state.pc_written = false;

        exit_halt(&mut *self.memory, false)?;

        // We assume that the core is running now
        self.set_core_status(CoreStatus::Running);

        Ok(())
    }

    fn reset(&mut self) -> Result<(), Error> {
        self.state.semihosting_command = None;
        self.state.clear_pending_step();
        self.state.pc_written = false;

        self.sequence
            .reset_system(&mut *self.memory, self.core_type, None)?;
        // Invalidate cached state: chip reset clears FP_CTRL and core status
        self.set_core_status(CoreStatus::Unknown);
        self.state.hw_breakpoints_enabled = false;
        Ok(())
    }

    fn reset_and_halt(&mut self, _timeout: Duration) -> Result<CoreInformation, Error> {
        // Set the vc_corereset bit in the DEMCR register.
        // This will halt the core after reset.
        self.reset_catch_set()?;
        self.state.clear_pending_step();
        self.state.pc_written = false;

        self.sequence
            .reset_system(&mut *self.memory, self.core_type, None)?;

        // Invalidate cached state: chip reset clears FP_CTRL and core status
        self.set_core_status(CoreStatus::Unknown);
        self.state.hw_breakpoints_enabled = false;

        // Some processors may not enter the halt state immediately after clearing the reset state.
        // Particularly: on PSOC 6, vector catch takes effect after the core's boot ROM finishes
        // executing, when jumping to the reset vector of the user application.
        match self.wait_for_core_halted(Duration::from_millis(100)) {
            Ok(()) => (),
            Err(Error::Arm(ArmError::Timeout)) if self.status()? == CoreStatus::Sleeping => {
                // On PSOC 6, if no application is loaded in flash, or if this core is waiting for
                // another core to boot it, the boot ROM sleeps and vector catch is not triggered.
                tracing::warn!(
                    "reset_and_halt timed out and core is sleeping; assuming core is quiescent"
                );
                self.halt(Duration::from_millis(100))?;
            }
            Err(e) => return Err(e),
        }

        const XPSR_THUMB: u32 = 1 << 24;

        let xpsr_value: u32 = self.read_core_reg(XPSR.id())?.try_into()?;
        if xpsr_value & XPSR_THUMB == 0 {
            self.write_core_reg(XPSR.id(), (xpsr_value | XPSR_THUMB).into())?;
        }

        self.reset_catch_clear()?;

        // try to read the program counter
        let pc_value = self.read_core_reg(self.program_counter().into())?;

        // get pc
        Ok(CoreInformation {
            pc: pc_value.try_into()?,
        })
    }

    fn step(&mut self) -> Result<CoreInformation, Error> {
        // First check if we stopped on a breakpoint, because this requires special handling before we can continue.
        let breakpoint_at_pc = if matches!(
            self.state.current_state,
            CoreStatus::Halted(HaltReason::Breakpoint(_))
        ) {
            let pc_before_step = self.read_core_reg(self.program_counter().into())?;
            self.enable_breakpoints(false)?;
            Some(pc_before_step)
        } else {
            None
        };

        // Only arm the pending step once the write has landed.
        exit_halt(&mut *self.memory, true)?;
        self.state.begin_step();

        // The single-step might put the core in lockup state. Lockup isn't considered "halted"
        // so we can't use `wait_for_core_halted` here.
        // So we wait for halted OR lockup, and if we entered lockup we halt.
        if let Err(err) = self.wait_for_status(Duration::from_millis(100), |s| {
            matches!(s, CoreStatus::Halted(_) | CoreStatus::LockedUp)
        }) {
            self.state.clear_pending_step();
            return Err(err);
        }
        if self.status()? == CoreStatus::LockedUp {
            self.halt(Duration::from_millis(100))?;
        }

        // Try to read the new program counter.
        let mut pc_after_step = self.read_core_reg(self.program_counter().into())?;

        // Re-enable breakpoints before we continue.
        if let Some(pc_before_step) = breakpoint_at_pc {
            // If we were stopped on a software breakpoint, then we need to manually advance the PC, or else we will be stuck here forever.
            if pc_before_step == pc_after_step
                && !self
                    .hw_breakpoints()?
                    .contains(&pc_before_step.try_into().ok())
            {
                tracing::debug!(
                    "Encountered a breakpoint instruction @ {}. We need to manually advance the program counter to the next instruction.",
                    pc_after_step
                );
                // Advance the program counter by the architecture specific byte size of the BKPT instruction.
                pc_after_step.increment_address(2)?;
                self.write_core_reg(self.program_counter().into(), pc_after_step)?;
            }
            self.enable_breakpoints(true)?;
        }

        self.state.semihosting_command = None;

        // The core halted again, and any PC write above was this function's own doing.
        self.state.pc_written = false;

        Ok(CoreInformation {
            pc: pc_after_step.try_into()?,
        })
    }

    fn read_core_reg(&mut self, address: RegisterId) -> Result<RegisterValue, Error> {
        if self.state.current_state.is_halted() {
            let val = read_core_reg(&mut *self.memory, address)?;
            Ok(val.into())
        } else {
            Err(Error::Arm(ArmError::CoreNotHalted))
        }
    }

    fn write_core_reg(&mut self, address: RegisterId, value: RegisterValue) -> Result<(), Error> {
        if self.state.current_state.is_halted() {
            write_core_reg(&mut *self.memory, address, value.try_into()?)?;
            if address == self.program_counter().id() {
                self.state.pc_written = true;
            }
            Ok(())
        } else {
            Err(Error::Arm(ArmError::CoreNotHalted))
        }
    }

    fn available_breakpoint_units(&mut self) -> Result<u32, Error> {
        V::available_breakpoint_units(&mut *self.memory)
    }

    /// See docs on the [`CoreInterface::hw_breakpoints`] trait
    fn hw_breakpoints(&mut self) -> Result<Vec<Option<u64>>, Error> {
        V::hw_breakpoints(&mut *self.memory)
    }

    fn enable_breakpoints(&mut self, state: bool) -> Result<(), Error> {
        V::enable_breakpoints(&mut *self.memory, state)?;
        self.state.hw_breakpoints_enabled = state;

        Ok(())
    }

    fn set_hw_breakpoint(&mut self, bp_register_index: usize, addr: u64) -> Result<(), Error> {
        V::set_hw_breakpoint(&mut *self.memory, bp_register_index, addr)
    }

    fn clear_hw_breakpoint(&mut self, bp_unit_index: usize) -> Result<(), Error> {
        V::clear_hw_breakpoint(&mut *self.memory, bp_unit_index)
    }

    fn registers(&self) -> &'static CoreRegisters {
        self.variant.registers(self.state)
    }

    fn program_counter(&self) -> &'static CoreRegister {
        &PC
    }

    fn frame_pointer(&self) -> &'static CoreRegister {
        &FP
    }

    fn stack_pointer(&self) -> &'static CoreRegister {
        &SP
    }

    fn return_address(&self) -> &'static CoreRegister {
        &RA
    }

    fn hw_breakpoints_enabled(&self) -> bool {
        self.state.hw_breakpoints_enabled
    }

    fn architecture(&self) -> Architecture {
        Architecture::Arm
    }

    fn core_type(&self) -> CoreType {
        self.core_type
    }

    fn instruction_set(&mut self) -> Result<InstructionSet, Error> {
        Ok(InstructionSet::Thumb2)
    }

    fn fpu_support(&mut self) -> Result<bool, Error> {
        Ok(self.state.fp_present)
    }

    fn floating_point_register_count(&mut self) -> Result<usize, Error> {
        Ok(V::FP_REGISTER_COUNT)
    }

    #[tracing::instrument(skip(self))]
    fn reset_catch_set(&mut self) -> Result<(), Error> {
        self.sequence
            .reset_catch_set(&mut *self.memory, self.core_type, None)?;

        Ok(())
    }

    #[tracing::instrument(skip(self))]
    fn reset_catch_clear(&mut self) -> Result<(), Error> {
        self.sequence
            .reset_catch_clear(&mut *self.memory, self.core_type, None)?;

        Ok(())
    }

    #[tracing::instrument(skip(self))]
    fn debug_core_stop(&mut self) -> Result<(), Error> {
        self.sequence
            .debug_core_stop(&mut *self.memory, self.core_type)?;

        Ok(())
    }

    #[tracing::instrument(skip(self))]
    fn enable_vector_catch(&mut self, condition: VectorCatchCondition) -> Result<(), Error> {
        self.variant
            .enable_vector_catch(&mut *self.memory, condition)
    }

    fn disable_vector_catch(&mut self, condition: VectorCatchCondition) -> Result<(), Error> {
        self.variant
            .disable_vector_catch(&mut *self.memory, condition)
    }
}

impl<V: CortexMVariant> CoreMemoryInterface for CortexM<'_, V> {
    type ErrorType = ArmError;

    fn memory(&self) -> &dyn MemoryInterface<Self::ErrorType> {
        self.memory.as_ref()
    }
    fn memory_mut(&mut self) -> &mut dyn MemoryInterface<Self::ErrorType> {
        self.memory.as_mut()
    }
}
