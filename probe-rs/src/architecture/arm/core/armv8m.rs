//! Register types and the core interface for armv8-M

use super::{
    CortexMState,
    cortex_m::{CortexM, CortexMVariant, IdPfr1, Mvfr0, enable_halting_debug},
    registers::armv8m::{
        V8M_BASE_SEC_FP_REGISTERS, V8M_BASE_SEC_REGISTERS, V8M_MAIN_FP_REGISTERS,
        V8M_MAIN_REGISTERS, V8M_MAIN_SEC_FP_REGISTERS, V8M_MAIN_SEC_REGISTERS,
    },
    registers::cortex_m::{CORTEX_M_CORE_REGISTERS, CORTEX_M_WITH_FP_CORE_REGISTERS},
};
use crate::{
    MemoryMappedRegister,
    architecture::arm::{ArmError, memory::ArmMemoryInterface},
    core::{CoreRegisters, VectorCatchCondition},
    error::Error,
    memory::valid_32bit_address,
};
use bitfield::bitfield;
use std::mem::size_of;

bitfield! {
    /// Debug Halting Control and Status Register, DHCSR (see armv8-M Architecture Reference Manual D1.2.38)
    ///
    /// To write this register successfully, you need to set the debug key via [`Dhcsr::enable_write`] first!
    #[derive(Copy, Clone)]
    pub struct Dhcsr(u32);
    impl Debug;
    /// Restart sticky status. Indicates the PE has processed a request to clear DHCSR.C_HALT to 0. That is, either
    /// a write to DHCSR that clears DHCSR.C_HALT from 1 to 0, or an External Restart Request.
    ///
    /// The possible values of this bit are:
    ///
    /// `0`: PE has not left Debug state since the last read of DHCSR.\
    /// `1`: PE has left Debug state since the last read of DHCSR.
    ///
    /// If the PE is not halted when `C_HALT` is cleared to zero, it is UNPREDICTABLE whether this bit is set to `1`. If
    /// `DHCSR.C_DEBUGEN == 0` this bit reads as an UNKNOWN value.
    ///
    /// This bit clears to zero when read.
    ///
    /// **Note**
    ///
    /// If the request to clear C_HALT is made simultaneously with a request to set C_HALT, for example
    /// a restart request and external debug request occur together, then the
    pub s_restart_st, _ : 26;
    ///  Indicates whether the processor has been reset since the last read of DHCSR:
    ///
    /// `0`: No reset since last DHCSR read.\
    /// `1`: At least one reset since last DHCSR read.
    ///
    /// This is a sticky bit, that clears to `0` on a read of DHCSR.
    pub s_reset_st, _: 25;
    /// When not in Debug state, indicates whether the processor has completed
    /// the execution of an instruction since the last read of DHCSR:
    ///
    /// `0`: No instruction has completed since last DHCSR read.\
    /// `1`: At least one instructions has completed since last DHCSR read.
    ///
    /// This is a sticky bit, that clears to `0` on a read of DHCSR.
    ///
    /// This bit is UNKNOWN:
    ///
    /// - after a Local reset, but is set to `1` as soon as the processor completes
    /// execution of an instruction.
    /// - when S_LOCKUP is set to `1`.
    /// - when S_HALT is set to `1`.
    ///
    /// When the processor is not in Debug state, a debugger can check this bit to
    /// determine if the processor is stalled on a load, store or fetch access.
    pub s_retire_st, _: 24;
    /// Floating-point registers Debuggable.
    /// Indicates that FPSCR, VPR, and the Floating-point registers are RAZ/WI in the current PE state when accessed via DCRSR. This reflects !CanDebugAccessFP().
    /// The possible values of this bit are:
    ///
    /// `0`: Floating-point registers accessible.\
    /// `1`: Floating-point registers are RAZ/WI.
    ///
    /// If version Armv8.1-M of the architecture is not implemented, this bit is RES0
    pub s_fpd, _: 23;
    /// Secure unprivileged halting debug enabled. Indicates whether Secure unprivileged-only halting debug is allowed or active.
    /// The possible values of this bit are:
    ///
    /// `0`: Secure invasive halting debug prohibited or not restricted to an unprivileged mode.\
    /// `1`: Unprivileged Secure invasive halting debug enabled.
    ///
    /// If the PE is in Non-debug state, this bit reflects the value of `UnprivHaltingDebugAllowed(TRUE) && !SecureHaltingDebugAllowed()`.
    ///
    /// The value of this bit does not change whilst the PE remains in Debug state.
    ///
    /// If the Security Extension is not implemented, this bit is RES0.
    /// If version Armv8.1 of the architecture and UDE are not implemented, this bit is RES0.
    pub s_suide, _: 22;
    /// Non-secure unprivileged halting debug enabled. Indicates whether Non-secure unprivileged-only halting debug is allowed or active.
    ///
    /// The possible values of this bit are:
    ///
    /// `0`: Non-secure invasive halting debug prohibited or not restricted to an unprivileged mode.\
    /// `1`: Unprivileged Non-secure invasive halting debug enabled.
    ///
    /// If the PE is in Non-debug state, this bit reflects the value of `UnprivHaltingDebugAllowed(FALSE) &&
    /// !HaltingDebugAllowed()`.
    ///
    /// The value of this bit does not change whilst the PE remains in Debug state.
    /// If version Armv8.1 of the architecture and UDE are not implemented, this bit is RES0
    pub s_nsuide, _: 21;
    /// Secure debug enabled. Indicates whether Secure invasive debug is allowed.
    /// The possible values of this bit are:
    ///
    /// `0`: Secure invasive debug prohibited.\
    /// `1`: Secure invasive debug allowed.
    ///
    /// If the PE is in Non-debug state, this bit reflects the value of SecureHaltingDebugAllowed() or UnprivHaltingDebugAllowed(TRUE).
    ///
    /// The value of this bit does not change while the PE remains in Debug state.
    ///
    /// If the Security Extension is not implemented, this bit is RES0.
    pub s_sde, _: 20;
    /// Indicates whether the processor is locked up because of an unrecoverable
    /// exception:
    ///
    /// `0` Not locked up.\
    /// `1` Locked up.
    /// See Unrecoverable exception cases on page B1-206 for more
    /// information.
    ///
    /// This bit can only read as `1` when accessed by a remote debugger using the
    /// DAP. The value of `1` indicates that the processor is running but locked up.
    /// The bit clears to `0` when the processor enters Debug state.
    pub s_lockup, _: 19;
    /// Indicates whether the processor is sleeping:
    ///
    /// `0` Not sleeping.
    /// `1` Sleeping.
    ///
    /// The debugger must set the DHCSR.C_HALT bit to `1` to gain control, or
    /// wait for an interrupt or other wakeup event to wakeup the system
    pub s_sleep, _: 18;
    /// Indicates whether the processor is in Debug state:
    ///
    /// `0`: Not in Debug state.\
    /// `1`: In Debug state.
    pub s_halt, _: 17;
    /// A handshake flag for transfers through the DCRDR:
    ///
    /// - Writing to DCRSR clears the bit to `0`.\
    /// - Completion of the DCRDR transfer then sets the bit to `1`.
    ///
    /// For more information about DCRDR transfers see Debug Core Register
    /// Data Register, DCRDR on page C1-292.
    ///
    /// `0`: There has been a write to the DCRDR, but the transfer is not complete.\
    /// `1` The transfer to or from the DCRDR is complete.
    ///
    /// This bit is only valid when the processor is in Debug state, otherwise the
    /// bit is UNKNOWN.
    pub s_regrdy, _: 16;
    /// Halt on PMU overflow control. Request entry to Debug state when a PMU counter overflows.
    ///
    /// The possible values of this bit are:
    ///
    /// `0`: No action.\
    /// `1`: If C_DEBUGEN is set to `1`, then when a PMU counter is configured to generate an interrupt overflows,
    /// the PE sets DHCSR.C_HALT to `1` and DFSR.PMU to `1`.
    ///
    /// PMU_OVSSET and PMU_OVSCLR indicate which counter or counters triggered the halt.
    ///
    /// If the Main Extension is not implemented, this bit is RES0.
    ///
    /// If version Armv8.1 of the architecture and PMU are not implemented, this bit is RES0.
    ///
    /// This bit resets to zero on a Cold reset.
    pub c_pmov, set_c_pmov: 6;
    /// Allow imprecise entry to Debug state. The actions on writing to this bit are:
    ///
    /// `0`: No action.\
    /// `1`: Allow imprecise entry to Debug state, for example by forcing any stalled load
    /// or store instruction to complete.
    ///
    /// Setting this bit to `1` allows a debugger to request imprecise entry to Debug state.
    ///
    /// The effect of setting this bit to `1` is UNPREDICTABLE unless the DHCSR write also sets
    /// C_DEBUGEN and C_HALT to `1`. This means that if the processor is not already in Debug
    /// state it enters Debug state when the stalled instruction completes.
    ///
    /// Writing `1` to this bit makes the state of the memory system UNPREDICTABLE. Therefore, if a
    /// debugger writes `1` to this bit it must reset the processor before leaving Debug state.
    ///
    /// **Note**
    ///
    /// - A debugger can write to the DHCSR to clear this bit to `0`. However, this does not
    /// remove the UNPREDICTABLE state of the memory system caused by setting C_SNAPSTALL to `1`.
    /// - The architecture does not guarantee that setting this bit to 1 will force entry to Debug
    /// state.
    /// - Arm strongly recommends that a value of `1` is never written to C_SNAPSTALL when
    /// the processor is in Debug state.
    ///
    /// A power-on reset sets this bit to `0`.
    pub c_snapstall, set_c_snapstall: 5;
    /// When debug is enabled, the debugger can write to this bit to mask
    /// PendSV, SysTick and external configurable interrupts:
    ///
    /// `0`: Do not mask.\
    /// `1` Mask PendSV, SysTick and external configurable interrupts.
    /// The effect of any attempt to change the value of this bit is UNPREDICTABLE
    /// unless both:
    /// - before the write to DHCSR, the value of the C_HALT bit is `1`.
    /// - the write to the DHCSR that changes the C_MASKINTS bit also
    /// writes `1` to the C_HALT bit.
    ///
    /// This means that a single write to DHCSR cannot set the C_HALT to `0` and
    /// change the value of the C_MASKINTS bit.
    ///
    /// The bit does not affect NMI. When DHCSR.C_DEBUGEN is set to `0`, the
    /// value of this bit is UNKNOWN.
    ///
    /// For more information about the use of this bit see Table C1-9 on
    /// page C1-282.
    ///
    /// This bit is UNKNOWN after a power-on reset.
    pub c_maskints, set_c_maskints: 3;
    /// Processor step bit. The effects of writes to this bit are:
    ///
    /// `0`: Single-stepping disabled.\
    /// `1`: Single-stepping enabled.
    ///
    /// For more information about the use of this bit see Table C1-9 on page C1-282.
    ///
    /// This bit is UNKNOWN after a power-on reset.
    pub c_step, set_c_step: 2;
    /// Processor halt bit. The effects of writes to this bit are:
    ///
    /// `0`: Request a halted processor to run.\
    /// `1`: Request a running processor to halt.
    ///
    /// Table C1-9 on page C1-282 shows the effect of writes to this bit when the
    /// processor is in Debug state.
    ///
    /// This bit is 0 after a System reset
    pub c_halt, set_c_halt: 1;
    /// Halting debug enable bit:
    /// `0`: Halting debug disabled.\
    /// `1`: Halting debug enabled.
    ///
    /// If a debugger writes to DHCSR to change the value of this bit from `0` to
    /// `1`, it must also write 0 to the C_MASKINTS bit, otherwise behavior is UNPREDICTABLE.
    ///
    /// This bit can only be written from the DAP. Access to the DHCSR from
    /// software running on the processor is IMPLEMENTATION DEFINED.
    ///
    /// However, writes to this bit from software running on the processor are ignored.
    ///
    /// This bit is `0` after a power-on reset.
    pub c_debugen, set_c_debugen: 0;
}

impl Dhcsr {
    /// This function sets the bit to enable writes to this register.
    pub fn enable_write(&mut self) {
        self.0 &= !(0xffff << 16);
        self.0 |= 0xa05f << 16;
    }
}

impl From<u32> for Dhcsr {
    fn from(value: u32) -> Self {
        Self(value)
    }
}

impl From<Dhcsr> for u32 {
    fn from(value: Dhcsr) -> Self {
        value.0
    }
}

impl MemoryMappedRegister<u32> for Dhcsr {
    const ADDRESS_OFFSET: u64 = 0xE000_EDF0;
    const NAME: &'static str = "DHCSR";
}

bitfield! {
    /// Application Interrupt and Reset Control Register, AIRCR (see armv8-M Architecture Reference Manual D1.2.3)
    ///
    /// [`Aircr::vectkey`] must be called before this register can effectively be written!
    #[derive(Copy, Clone)]
    pub struct Aircr(u32);
    impl Debug;
    /// Vector Key. The value `0x05FA` must be written to this register, otherwise
    /// the register write is UNPREDICTABLE.
    get_vectkeystat, set_vectkey: 31,16;
    /// Indicates the memory system data endianness:
    ///
    /// `0`: little endian.\
    /// `1` big endian.
    ///
    /// See Endian support on page A3-44 for more information.
    pub endianness, set_endianness: 15;
    /// Priority grouping, indicates the binary point position.
    /// For information about the use of this field see Priority grouping on page B1-527.
    ///
    /// This field resets to `0b000`.
    pub prigroup, set_prigroup: 10,8;
    /// System reset request Secure only. The value of this bit defines whether the SYSRESETREQ bit is functional for Non-secure use.
    /// This bit is not banked between Security states.
    /// The possible values of this bit are:
    ///
    /// `0`: SYSRESETREQ functionality is available to both Security states.\
    /// `1`: SYSRESETREQ functionality is only available to Secure state.
    ///
    /// This bit is RAZ/WI from Non-secure state.
    /// This bit resets to zero on a Warm reset
    pub sysresetreqs, set_sysresetreqs: 3;
    ///  System Reset Request:
    ///
    /// `0` do not request a reset.\
    /// `1` request reset.
    ///
    /// Writing 1 to this bit asserts a signal to request a reset by the external
    /// system. The system components that are reset by this request are
    /// IMPLEMENTATION DEFINED. A Local reset is required as part of a system
    /// reset request.
    ///
    /// A Local reset clears this bit to `0`.
    ///
    /// See Reset management on page B1-208 for more information
    pub sysresetreq, set_sysresetreq: 2;
    /// Clears all active state information for fixed and configurable exceptions:
    ///
    /// `0`: do not clear state information.\
    /// `1`: clear state information.
    ///
    /// The effect of writing a `1` to this bit if the processor is not halted in Debug
    /// state is UNPREDICTABLE.
    pub vectclractive, set_vectclractive: 1;
    /// Writing `1` to this bit causes a local system reset, see Reset management on page B1-559 for
    /// more information. This bit self-clears.
    ///
    /// The effect of writing a `1` to this bit if the processor is not halted in Debug state is
    /// UNPREDICTABLE.
    ///
    /// When the processor is halted in Debug state, if a write to the register writes a `1` to both
    /// VECTRESET and SYSRESETREQ, the behavior is UNPREDICTABLE.
    ///
    /// This bit is write only.
    pub vectreset, set_vectreset: 0;
}

impl From<u32> for Aircr {
    fn from(value: u32) -> Self {
        Self(value)
    }
}

impl From<Aircr> for u32 {
    fn from(value: Aircr) -> Self {
        value.0
    }
}

impl Aircr {
    /// Must be called before writing the register.
    pub fn vectkey(&mut self) {
        self.set_vectkey(0x05FA);
    }

    /// Verifies that the vector key is correct (see [`Aircr::vectkey`])
    pub fn vectkeystat(&self) -> bool {
        self.get_vectkeystat() == 0xFA05
    }
}

impl MemoryMappedRegister<u32> for Aircr {
    const ADDRESS_OFFSET: u64 = 0xE000_ED0C;
    const NAME: &'static str = "AIRCR";
}

/// Debug Core Register Data Register, DCRDR (see armv8-M Architecture Reference Manual D1.2.32)
#[derive(Debug, Copy, Clone)]
pub struct Dcrdr(u32);

impl From<u32> for Dcrdr {
    fn from(value: u32) -> Self {
        Self(value)
    }
}

impl From<Dcrdr> for u32 {
    fn from(value: Dcrdr) -> Self {
        value.0
    }
}

impl MemoryMappedRegister<u32> for Dcrdr {
    const ADDRESS_OFFSET: u64 = 0xE000_EDF8;
    const NAME: &'static str = "DCRDR";
}

bitfield! {
    /// /// Debug Exception and Monitor Control Register, DEMCR (see armv8-M Architecture Reference Manual D1.2.36)
    #[derive(Copy, Clone)]
    pub struct Demcr(u32);
    impl Debug;
    /// Global enable for DWT, PMU and ITM features
    pub trcena, set_trcena: 24;
    /// Monitor pending request key. Writes to the mon_pend and mon_en fields
    /// request are ignored unless `monprkey` is set to zero concurrently.
    pub monprkey, set_monprkey: 23;
    /// Unprivileged monitor enable.
    pub umon_en, set_umon_en: 21;
    /// Secure DebugMonitor enable
    pub sdme, set_sdme: 20;
    /// DebugMonitor semaphore bit
    pub mon_req, set_mon_req: 19;
    /// Step the processor?
    pub mon_step, set_mon_step: 18;
    /// Sets or clears the pending state of the DebugMonitor exception
    pub mon_pend, set_mon_pend: 17;
    /// Enable the DebugMonitor exception
    pub mon_en, set_mon_en: 16;
    /// Enable halting debug on a SecureFault exception
    pub vc_sferr, set_vc_sferr: 11;
    /// Enable halting debug trap on a HardFault exception
    pub vc_harderr, set_vc_harderr: 10;
    /// Enable halting debug trap on a fault occurring during exception entry
    /// or exception return
    pub vc_interr, set_vc_interr: 9;
    /// Enable halting debug trap on a BusFault exception
    pub vc_buserr, set_vc_buserr: 8;
    /// Enable halting debug trap on a UsageFault exception caused by a state
    /// information error, for example an Undefined Instruction exception
    pub vc_staterr, set_vc_staterr: 7;
    /// Enable halting debug trap on a UsageFault exception caused by a
    /// checking error, for example an alignment check error
    pub vc_chkerr, set_vc_chkerr: 6;
    /// Enable halting debug trap on a UsageFault caused by an access to a
    /// Coprocessor
    pub vc_nocperr, set_vc_nocperr: 5;
    /// Enable halting debug trap on a MemManage exception.
    pub vc_mmerr, set_vc_mmerr: 4;
    /// Enable Reset Vector Catch
    pub vc_corereset, set_vc_corereset: 0;
}

impl From<u32> for Demcr {
    fn from(value: u32) -> Self {
        Self(value)
    }
}

impl From<Demcr> for u32 {
    fn from(value: Demcr) -> Self {
        value.0
    }
}

impl MemoryMappedRegister<u32> for Demcr {
    const ADDRESS_OFFSET: u64 = 0xe000_edfc;
    const NAME: &'static str = "DEMCR";
}

bitfield! {
    /// Flash Patch Control Register, FP_CTRL (see armv8-M Architecture Reference Manual D1.2.108)
    #[derive(Copy,Clone)]
    pub struct FpCtrl(u32);
    impl Debug;
    /// Flash Patch breakpoint architecture revision:
    /// 0000 Flash Patch breakpoint version 1.
    /// 0001 Flash Patch breakpoint version 2. Supports breakpoints on any location in the 4GB address range.
    pub rev, _: 31, 28;
    num_code_1, _: 14, 12;
    /// The number of literal address comparators supported, starting from NUM_CODE upwards.
    /// UNK/SBZP if Flash Patch is not implemented. Flash Patch is not implemented if `FP_REMAP[29]` is 0.
    /// If this field is zero, the implementation does not support literal comparators.
    pub num_lit, _: 11, 8;
    num_code_0, _: 7, 4;
    /// On any write to FP_CTRL, this bit must be 1. A write to the register with this bit set to zero
    /// is ignored. The Flash Patch Breakpoint unit ignores the write unless this bit is 1.
    pub _, set_key: 1;
    /// Enable bit for the FPB:
    /// 0 Flash Patch breakpoint disabled.
    /// 1 Flash Patch breakpoint enabled.
    /// A power-on reset clears this bit to 0.
    pub enable, set_enable: 0;
}

impl FpCtrl {
    /// The number of instruction address comparators.
    /// If NUM_CODE is zero, the implementation does not support any instruction address comparators.
    pub fn num_code(&self) -> u32 {
        (self.num_code_1() << 4) | self.num_code_0()
    }
}

impl MemoryMappedRegister<u32> for FpCtrl {
    const ADDRESS_OFFSET: u64 = 0xE000_2000;
    const NAME: &'static str = "FP_CTRL";
}

impl From<u32> for FpCtrl {
    fn from(value: u32) -> Self {
        FpCtrl(value)
    }
}

impl From<FpCtrl> for u32 {
    fn from(value: FpCtrl) -> Self {
        value.0
    }
}

bitfield! {
    /// FP_COMPn, Flash Patch Comparator Register, n = 0 - 125 (see armv8-M Architecture Reference Manual D1.2.107)
    #[derive(Copy,Clone)]
    pub struct FpCompN(u32);
    impl Debug;
    /// BPADDR, `bits[31:1]` Breakpoint address. Specifies bits`[31:1]` of the breakpoint instruction address.
    /// If BE == 0, this field is Reserved, UNK/SBZP.
    /// The reset value of this field is UNKNOWN.
    pub bp_addr, set_bp_addr: 31, 1;
    /// Enable bit for breakpoint:
    /// 0 Breakpoint disabled.
    /// 1 Breakpoint enabled.
    /// The reset value of this bit is UNKNOWN.
    pub enable, set_enable: 0;
}

impl MemoryMappedRegister<u32> for FpCompN {
    const ADDRESS_OFFSET: u64 = 0xE000_2008;
    const NAME: &'static str = "FP_COMPn";
}

impl From<u32> for FpCompN {
    fn from(value: u32) -> Self {
        FpCompN(value)
    }
}

impl From<FpCompN> for u32 {
    fn from(value: FpCompN) -> Self {
        value.0
    }
}

/// The ARMv8-M core driver.
pub type Armv8m<'probe> = CortexM<'probe, Armv8mVariant>;

/// ARMv8-M adds the Security Extension, which selects the register set and
/// gates the SecureFault vector catch.
pub struct Armv8mVariant {
    /// True if the core implements the Security Extension.
    security: bool,
}

impl Armv8mVariant {
    /// Set or clear a vector catch condition, including VC_SFERR.
    fn vector_catch(
        memory: &mut dyn ArmMemoryInterface,
        condition: VectorCatchCondition,
        enable: bool,
    ) -> Result<(), Error> {
        let mut demcr = Demcr(memory.read_word_32(Demcr::get_mmio_address())?);
        let idpfr1 = IdPfr1(memory.read_word_32(IdPfr1::get_mmio_address())?);
        match condition {
            VectorCatchCondition::HardFault => demcr.set_vc_harderr(enable),
            VectorCatchCondition::CoreReset => demcr.set_vc_corereset(enable),
            VectorCatchCondition::SecureFault => {
                if !idpfr1.security_present() {
                    return Err(Error::Arm(ArmError::ExtensionRequired(&["Security"])));
                }
                demcr.set_vc_sferr(enable);
            }
            VectorCatchCondition::All => {
                demcr.set_vc_harderr(enable);
                demcr.set_vc_corereset(enable);
                if idpfr1.security_present() {
                    demcr.set_vc_sferr(enable);
                }
            }
            VectorCatchCondition::Svc | VectorCatchCondition::Hlt => {
                return Err(Error::NotImplemented("vector catch condition Svc/Hlt"));
            }
        };

        memory.write_word_32(Demcr::get_mmio_address(), demcr.into())?;
        Ok(())
    }
}

impl CortexMVariant for Armv8mVariant {
    const FP_REGISTER_COUNT: usize = 32;

    fn new(memory: &mut dyn ArmMemoryInterface) -> Result<Self, Error> {
        // Read per attach rather than once into CortexMState, because the
        // register set is chosen from it on every call.
        let idpfr1 = IdPfr1(memory.read_word_32(IdPfr1::get_mmio_address())?);

        Ok(Self {
            security: idpfr1.security_present(),
        })
    }

    fn detect_fpu(
        memory: &mut dyn ArmMemoryInterface,
        state: &mut CortexMState,
    ) -> Result<(), Error> {
        state.fp_present = Mvfr0(memory.read_word_32(Mvfr0::get_mmio_address())?).fp_present();
        Ok(())
    }

    fn registers(&self, state: &CortexMState) -> &'static CoreRegisters {
        let main = true; // TODO m33 is mainline, no one has m23 (baseline) yet
        let security = self.security;
        let fp = state.fp_present;

        match (main, security, fp) {
            (true, true, true) => &V8M_MAIN_SEC_FP_REGISTERS,
            (true, true, false) => &V8M_MAIN_SEC_REGISTERS,
            (true, false, true) => &V8M_MAIN_FP_REGISTERS,
            (true, false, false) => &V8M_MAIN_REGISTERS,
            (false, true, true) => &V8M_BASE_SEC_FP_REGISTERS,
            (false, true, false) => &V8M_BASE_SEC_REGISTERS,
            (false, false, true) => &CORTEX_M_WITH_FP_CORE_REGISTERS,
            (false, false, false) => &CORTEX_M_CORE_REGISTERS,
        }
    }

    fn available_breakpoint_units(memory: &mut dyn ArmMemoryInterface) -> Result<u32, Error> {
        let raw_val = memory.read_word_32(FpCtrl::get_mmio_address())?;

        let reg = FpCtrl::from(raw_val);

        Ok(reg.num_code())
    }

    fn hw_breakpoints(memory: &mut dyn ArmMemoryInterface) -> Result<Vec<Option<u64>>, Error> {
        let mut breakpoints = vec![];
        let num_hw_breakpoints = Self::available_breakpoint_units(memory)? as usize;
        for bp_unit_index in 0..num_hw_breakpoints {
            let reg_addr = FpCompN::get_mmio_address() + (bp_unit_index * size_of::<u32>()) as u64;
            // The raw breakpoint address as read from memory
            let register_value = memory.read_word_32(reg_addr)?;
            // The breakpoint address after it has been adjusted for FpRev 1 or 2
            if FpCompN::from(register_value).enable() {
                let breakpoint = FpCompN::from(register_value).bp_addr() << 1;
                breakpoints.push(Some(breakpoint as u64));
            } else {
                breakpoints.push(None);
            }
        }
        Ok(breakpoints)
    }

    fn enable_breakpoints(memory: &mut dyn ArmMemoryInterface, enabled: bool) -> Result<(), Error> {
        let mut val = FpCtrl::from(0);
        val.set_key(true);
        val.set_enable(enabled);

        memory.write_word_32(FpCtrl::get_mmio_address(), val.into())?;
        memory.flush()?;

        Ok(())
    }

    fn set_hw_breakpoint(
        memory: &mut dyn ArmMemoryInterface,
        index: usize,
        addr: u64,
    ) -> Result<(), Error> {
        let addr = valid_32bit_address(addr)?;

        let mut val = FpCompN::from(0);

        // clear bits which cannot be set and shift into position
        let comp_val = (addr & 0xff_ff_ff_fe) >> 1;

        val.set_bp_addr(comp_val);
        val.set_enable(true);

        let reg_addr = FpCompN::get_mmio_address() + (index * size_of::<u32>()) as u64;

        memory.write_word_32(reg_addr, val.into())?;

        Ok(())
    }

    fn clear_hw_breakpoint(memory: &mut dyn ArmMemoryInterface, index: usize) -> Result<(), Error> {
        let mut val = FpCompN::from(0);
        val.set_enable(false);
        val.set_bp_addr(0);

        let reg_addr = FpCompN::get_mmio_address() + (index * size_of::<u32>()) as u64;

        memory.write_word_32(reg_addr, val.into())?;

        Ok(())
    }

    fn enable_vector_catch(
        &self,
        memory: &mut dyn ArmMemoryInterface,
        condition: VectorCatchCondition,
    ) -> Result<(), Error> {
        enable_halting_debug(memory)?;
        Self::vector_catch(memory, condition, true)
    }

    fn disable_vector_catch(
        &self,
        memory: &mut dyn ArmMemoryInterface,
        condition: VectorCatchCondition,
    ) -> Result<(), Error> {
        Self::vector_catch(memory, condition, false)
    }
}
