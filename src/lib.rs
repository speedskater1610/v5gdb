#![allow(missing_docs)]
#![cfg(target_arch = "arm")]

use std::sync::{Mutex, OnceLock};
use core::{any::Any, arch::asm};

use crate::{
    exceptions::{DebugEventContext, install_vectors},
    gdb_target::breakpoint::BreakpointError,
};

pub mod cpu;
pub mod debugger;
pub mod exceptions;
pub mod gdb_target;
pub mod transport;

pub static DEBUGGER: OnceLock<Mutex<&mut dyn Debugger>> = OnceLock::new();

/// Debugger implementation.
///
/// # Safety
///
/// The debugger must not corrupt the CPU state when handling debug events.
pub unsafe trait Debugger: Send + Any {
    /// Initializes the debugger.
    fn initialize(&mut self);

    /// Registers a breakpoint at the specified address.
    ///
    /// # Safety
    ///
    /// Breakpoints may only be placed on read/write/executable addresses which contain an
    /// instruction and are not used as data.
    ///
    /// # Errors
    ///
    /// This function will return an error if there are no more free breakpoint slots or if
    /// the specified address already has a breakpoint on it.
    unsafe fn register_breakpoint(&mut self, addr: u32, thumb: bool)
    -> Result<(), BreakpointError>;

    /// A callback function which is run whenever a breakpoint is triggered.
    ///
    /// The function is given access to the pre-breakpoint CPU state and can view/modify it as
    /// needed.
    ///
    /// # Safety
    ///
    /// The given fault must represent valid, saved CPU state.
    unsafe fn handle_debug_event(&mut self, ctx: &mut DebugEventContext);
}

/// Set the current debugger.
pub fn install(debugger: impl Debugger + 'static) {
    DEBUGGER
        .set(Mutex::new(Box::leak(Box::new(debugger))))
        .map_err(|_| ())
        .expect("A debugger is already installed.");

    install_vectors();

    DEBUGGER.get().unwrap().try_lock().unwrap().initialize();
}

#[allow(clippy::inline_always)]
#[inline(always)]
pub fn breakpoint() {
    unsafe {
        asm!("bkpt", options(nostack, nomem, preserves_flags));
    }
}
