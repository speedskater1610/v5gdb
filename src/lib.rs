#![allow(missing_docs)]
#![no_std]
#![cfg_attr(not(target_arch = "arm"), allow(unused))]

#[cfg(feature = "alloc")]
extern crate alloc;

use core::any::Any;

use spin::{Mutex, Once};

use crate::exceptions::DebugEventContext;

pub mod cpu;
#[cfg(target_arch = "arm")]
pub mod debugger;
pub mod exceptions;
#[cfg(target_arch = "arm")]
pub mod gdb_target;
#[cfg(target_arch = "arm")]
mod sdk;
pub mod transport;

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

pub static DEBUGGER: Once<Mutex<&mut dyn Debugger>> = Once::new();

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
///
/// This will move the given debugger onto the heap, so it's more expensive than [`install_by_ref`].
#[cfg(feature = "alloc")]
pub fn install(debugger: impl Debugger + 'static) {
    use alloc::boxed::Box;
    install_by_ref(Box::leak(Box::new(debugger)));
}

/// Set the current debugger, by reference.
pub fn install_by_ref(debugger: &'static mut dyn Debugger) {
    assert!(!DEBUGGER.is_completed(), "A debugger is already installed.");
    DEBUGGER.call_once(|| Mutex::new(debugger));

    #[cfg(target_arch = "arm")]
    exceptions::install_vectors();

    DEBUGGER.get().unwrap().try_lock().unwrap().initialize();
}

/// Manually trigger a breakpoint.
///
/// This should only be run if a debugger is installed. If no debugger is installed, this will
/// crash your program instead of pausing it.
#[macro_export]
macro_rules! breakpoint {
    () => {
        #[cfg(target_arch = "arm")]
        unsafe {
            ::core::arch::asm!("bkpt", options(nostack, nomem, preserves_flags));
        }
    };
}
