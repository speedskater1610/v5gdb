#![allow(missing_docs)]
#![cfg_attr(not(target_arch = "arm"), allow(unused))]

use core::{any::Any, arch::asm};
use std::sync::{Mutex, OnceLock};

use crate::exceptions::DebugEventContext;

pub mod cpu;
#[cfg(target_arch = "arm")]
pub mod debugger;
pub mod exceptions;
#[cfg(target_arch = "arm")]
pub mod gdb_target;
pub mod transport;
#[cfg(target_arch = "arm")]
mod sdk;

#[cfg(not(target_arch = "arm"))]
pub mod debugger {
    use crate::{Debugger, transport::Transport};

    pub struct V5Debugger<S: Transport> {
        _stream: S,
    }

    impl<S: Transport> V5Debugger<S> {
        /// Creates a new debugger.
        #[must_use]
        pub fn new(stream: S) -> Self {
            Self { _stream: stream }
        }
    }

    unsafe impl<S: Transport + 'static> Debugger for V5Debugger<S> {
        fn initialize(&mut self) {}

        unsafe fn handle_debug_event(&mut self, _ctx: &mut crate::exceptions::DebugEventContext) {
            unimplemented!()
        }
    }
}

pub static DEBUGGER: OnceLock<Mutex<&mut dyn Debugger>> = OnceLock::new();

/// Debugger implementation.
///
/// # Safety
///
/// The debugger must not corrupt the CPU state when handling debug events.
pub unsafe trait Debugger: Send + Any {
    /// Initializes the debugger.
    fn initialize(&mut self);

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

    #[cfg(target_arch = "arm")]
    exceptions::install_vectors();

    DEBUGGER.get().unwrap().try_lock().unwrap().initialize();
}

#[allow(clippy::inline_always)]
#[inline(always)]
pub fn breakpoint() {
    #[cfg(target_arch = "arm")]
    unsafe {
        asm!("bkpt", options(nostack, nomem, preserves_flags));
    }
}
