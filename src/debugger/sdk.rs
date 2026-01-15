//! Alternative implementations of SDK functions under the debugger.

use vex_sdk::*;

use crate::{debugger::V5Debugger, transport::Transport};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InternalBreakpoint {
    ExitRequest,
}

impl<S: Transport> V5Debugger<S> {
    pub(crate) fn register_internal_breakpoints(&mut self) {
        assert!(self.internal_breaks.is_none());

        let internal_breaks = [
            (
                InternalBreakpoint::ExitRequest,
                vexSystemExitRequest as *const () as u32,
            ),
        ];

        for (_id, addr) in internal_breaks {
            unsafe {
                self.target
                    .register_sw_breakpoint(addr, false, true)
                    .unwrap();
            }
        }

        self.internal_breaks = Some(internal_breaks);
    }

    /// Handle an internal hardware breakpoint, if applicable.
    ///
    /// Returns whether the debug console should be shown even if the user hasn't requested it.
    pub(crate) fn handle_internal_breakpoint(&mut self) -> bool {
        let pc = self.target.exception_ctx.program_counter;

        let Some(&(id, addr)) = self
            .internal_breaks
            .iter()
            .flatten()
            .find(|&&(_id, addr)| addr == pc)
        else {
            return false;
        };

        match id {
            InternalBreakpoint::ExitRequest => {
                self.target.exit_request();

                // Continue to the debug monitor - once GDB realizes we are exiting, it will
                // disconnect and allow us to return back to calling vexSystemExitRequest.
                self.target.remove_sw_breakpoint(addr, true);
                true
            }
        }
    }
}
