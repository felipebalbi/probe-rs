//! Ambiq vendor support.

use probe_rs_target::Chip;

use crate::{
    config::DebugSequence,
    vendor::{Vendor, ambiq::sequences::apollo::Apollo3},
};

pub mod sequences;

/// Ambiq
#[derive(docsplay::Display)]
pub struct Ambiq;

impl Vendor for Ambiq {
    fn try_create_debug_sequence(&self, chip: &Chip) -> Option<DebugSequence> {
        let sequence = if chip.name.starts_with("AMA3") {
            DebugSequence::Arm(Apollo3::create())
        } else {
            return None;
        };

        Some(sequence)
    }
}
