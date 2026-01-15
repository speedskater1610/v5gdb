//! Software breakpoint management.


use gdbstub::target::ext::breakpoints::{Breakpoints, HwBreakpointOps, SwBreakpointOps};
use snafu::Snafu;

use super::V5Target;
use crate::cpu::cache;

pub mod hardware;
pub mod software;

impl Breakpoints for V5Target {
    fn support_sw_breakpoint(&mut self) -> Option<SwBreakpointOps<'_, Self>> {
        Some(self)
    }

    fn support_hw_breakpoint(&mut self) -> Option<HwBreakpointOps<'_, Self>> {
        Some(self)
    }
}

impl V5Target {
    /// Enable or disable the triggering of breakpoints.
    pub fn set_breakpoints_ignored(&mut self, ignored: bool) {
        self.breaks_paused = ignored;
        for bkpt in self.breaks.iter_mut().flatten() {
            bkpt.set_enabled(!ignored);
            cache::sync_instruction(bkpt.cache_target());
        }
        // Hardware breakpoints never trigger in abort mode.
    }
}

#[derive(Debug, Snafu)]
pub enum BreakpointError {
    /// A software breakpoint can't be placed there because that region isn't writable.
    CannotWrite,
    /// There is already a breakpoint with this address.
    AlreadyExists,
    /// There are no free breakpoint slots.
    NoSpace,
    /// The specified breakpoint address is not aligned properly for the given instruction type.
    NotAlignedCorrectly,
}
