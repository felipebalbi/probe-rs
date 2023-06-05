use crate::{
    core::{BreakpointCause, RegisterValue},
    memory_mapped_bitfield_register, CoreStatus, HaltReason,
};

pub mod armv6m;
pub mod armv7a;
pub mod armv7m;
pub mod armv8a;
pub mod armv8m;

pub(crate) mod armv7a_debug_regs;
pub(crate) mod armv8a_debug_regs;
pub(crate) mod cortex_m;
pub(crate) mod instructions;
pub(crate) mod registers;

/// Core information data which is downloaded from the target, represents its state and can be used for debugging.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Dump {
    /// The register values at the time of the dump.
    pub regs: [u32; 16],
    stack_addr: u32,
    stack: Vec<u8>,
}

impl Dump {
    /// Create a new dump from a SP and a stack dump with zeroed out registers.
    pub fn new(stack_addr: u32, stack: Vec<u8>) -> Dump {
        Dump {
            regs: [0u32; 16],
            stack_addr,
            stack,
        }
    }
}

memory_mapped_bitfield_register! {
    pub struct Dfsr(u32);
    0xE000_ED30, "DFSR",
    pub external, set_external: 4;
    pub vcatch, set_vcatch: 3;
    pub dwttrap, set_dwttrap: 2;
    pub bkpt, set_bkpt: 1;
    pub halted, set_halted: 0;
}

impl Dfsr {
    fn clear_all() -> Self {
        Dfsr(0b11111)
    }

    /// This only returns the correct halt_reason for armv(x)-m variants. The armv(x)-a variants have their own implementation.
    // TODO: The different implementations between -m and -a can do with cleanup/refactoring.
    fn halt_reason(&self) -> HaltReason {
        if self.0 == 0 {
            // No bit is set
            HaltReason::Unknown
        } else if self.0.count_ones() > 1 {
            tracing::debug!("DFSR: {:?}", self);

            // We cannot identify why the chip halted,
            // it could be for multiple reasons.

            // For debuggers, it's important to know if
            // the core halted because of a breakpoint.
            // Because of this, we still return breakpoint
            // even if other reasons are possible as well.
            if self.bkpt() {
                HaltReason::Breakpoint(BreakpointCause::Unknown)
            } else {
                HaltReason::Multiple
            }
        } else if self.bkpt() {
            HaltReason::Breakpoint(BreakpointCause::Unknown)
        } else if self.external() {
            HaltReason::External
        } else if self.dwttrap() {
            HaltReason::Watchpoint
        } else if self.halted() {
            HaltReason::Request
        } else if self.vcatch() {
            HaltReason::Exception
        } else {
            // We check that exactly one bit is set, so we should hit one of the cases above.
            panic!("This should not happen. Please open a bug report.")
        }
    }
}

impl From<u32> for Dfsr {
    fn from(val: u32) -> Self {
        // Ensure that all unused bits are set to zero
        // This makes it possible to check the number of
        // set bits using count_ones().
        Dfsr(val & 0b11111)
    }
}

impl From<Dfsr> for u32 {
    fn from(register: Dfsr) -> Self {
        register.0
    }
}

#[derive(Debug)]
pub struct CortexMState {
    initialized: bool,

    hw_breakpoints_enabled: bool,

    current_state: CoreStatus,

    fp_present: bool,
}

impl CortexMState {
    pub(crate) fn new() -> Self {
        Self {
            initialized: false,
            hw_breakpoints_enabled: false,
            current_state: CoreStatus::Unknown,
            fp_present: false,
        }
    }

    fn initialize(&mut self) {
        self.initialized = true;
    }

    fn initialized(&self) -> bool {
        self.initialized
    }
}

#[derive(Debug)]
pub struct CortexAState {
    initialized: bool,

    current_state: CoreStatus,

    // Is the core currently in a 64-bit mode?
    is_64_bit: bool,

    register_cache: Vec<Option<(RegisterValue, bool)>>,

    // Number of floating point registers
    fp_reg_count: Option<usize>,
}

impl CortexAState {
    pub(crate) fn new() -> Self {
        Self {
            initialized: false,
            current_state: CoreStatus::Unknown,
            is_64_bit: false,
            register_cache: vec![],
            fp_reg_count: None,
        }
    }

    fn initialize(&mut self) {
        self.initialized = true;
    }

    fn initialized(&self) -> bool {
        self.initialized
    }
}

/// Core implementations should call this function when they
/// wish to update the [`CoreStatus`] of their core.
///
/// It will reflect the core status to the probe/memory interface if
/// the status has changed, and will replace `current_status` with
/// `new_status`.
pub fn update_core_status<
    P: super::memory::adi_v5_memory_interface::ArmProbe + ?Sized,
    T: core::ops::DerefMut<Target = P>,
>(
    probe: &mut T,
    current_status: &mut CoreStatus,
    new_status: CoreStatus,
) {
    if *current_status != new_status {
        probe.deref_mut().update_core_status(new_status);
    }
    *current_status = new_status;
}
