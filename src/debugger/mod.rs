//! Main debugger loop and event handling logic.

use std::{convert::Infallible, ops::ControlFlow};

use gdbstub::{
    common::Signal,
    stub::{
        GdbStubBuilder, GdbStubError, SingleThreadStopReason, state_machine::GdbStubStateMachine,
    },
    target::ext::breakpoints::HwBreakpoint,
};
use snafu::Snafu;
use vex_sdk::vexSystemExitRequest;
use zynq7000::devcfg::DevCfg;

use crate::{
    Debugger,
    cpu::debug::DebugEventReason,
    debugger::sdk::InternalBreakpoint,
    exceptions::DebugEventContext,
    gdb_target::{
        V5Target,
        arch::ArmBreakpointKind,
        breakpoint::{hardware::Specificity, software::SwBreakpoint},
    },
    transport::Transport,
};

pub mod sdk;

#[derive(Debug, Snafu)]
pub enum DebuggerError {
    #[snafu(context(false))]
    Io { source: std::io::Error },
    #[snafu(context(false))]
    GdbStub {
        source: GdbStubError<Infallible, std::io::Error>,
    },
}

/// Debugger state machine for handling remote connections.
pub struct V5Debugger<S: Transport> {
    target: V5Target,
    internal_breaks: Option<[(InternalBreakpoint, u32); 1]>,
    stream: S,
    gdb_buffer: Option<&'static mut [u8]>,
    gdb: Option<GdbStubStateMachine<'static, V5Target, S>>,
}

impl<S: Transport> V5Debugger<S> {
    /// Creates a new debugger.
    #[must_use]
    pub fn new(stream: S) -> Self {
        Self {
            target: V5Target::new(&mut unsafe { DevCfg::new_mmio_fixed() }),
            internal_breaks: None,
            stream,
            gdb_buffer: Some(Box::leak(vec![0; 0x2000].into_boxed_slice())),
            gdb: None,
        }
    }

    fn drive_state_machine<'a>(
        gdb: GdbStubStateMachine<'a, V5Target, S>,
        target: &mut V5Target,
    ) -> Result<GdbStubStateMachine<'a, V5Target, S>, DebuggerError> {
        match gdb {
            GdbStubStateMachine::Idle(mut gdb) => {
                if let Ok(byte) = gdb.borrow_conn().read() {
                    Ok(gdb.incoming_data(target, byte)?)
                } else {
                    Ok(gdb.into())
                }
            }
            GdbStubStateMachine::Running(gdb) => {
                let reported_reason = target.get_stop_reason();
                target.single_step_request = None;

                // Once we tell GDB we've exited we should exit the monitor because the session will
                // end.
                if matches!(reported_reason, SingleThreadStopReason::Exited(_)) {
                    target.resume = true;
                }

                Ok(gdb.report_stop(target, reported_reason)?)
            }
            GdbStubStateMachine::CtrlCInterrupt(gdb) => {
                let stop_reason: Option<SingleThreadStopReason<_>> = None;
                Ok(gdb.interrupt_handled(target, stop_reason)?)
            }
            GdbStubStateMachine::Disconnected(gdb) => Ok(gdb.return_to_idle()),
        }
    }

    /// Returns the debugger's internal state.
    #[must_use]
    pub const fn target(&mut self) -> &mut V5Target {
        &mut self.target
    }

    /// Runs the debug console until the user indicates they want to continue program execution.
    fn run_debug_console(&mut self) {
        // If this is the first time a breakpoint has happened, then we'll set up the state machine
        // for GDB.
        let mut gdb = self.gdb.take().unwrap_or_else(|| {
            let buffer = self.gdb_buffer.take().unwrap();
            let stub = GdbStubBuilder::new(self.stream.clone())
                .with_packet_buffer(buffer)
                .build()
                .unwrap();

            stub.run_state_machine(&mut self.target).unwrap()
        });

        // Enter debugging loop until it's time to resume.

        self.target.reset_resume();
        while !self.target.resume {
            std::thread::yield_now();

            gdb = Self::drive_state_machine(gdb, &mut self.target)
                .expect("debugger encountered an error");
        }

        self.target.resume = false;
        self.gdb = Some(gdb);
    }
}

unsafe impl<S: Transport + 'static> Debugger for V5Debugger<S> {
    fn initialize(&mut self) {
        self.register_internal_breakpoints();
    }

    unsafe fn register_breakpoint(
        &mut self,
        addr: u32,
        thumb: bool,
    ) -> Result<(), crate::gdb_target::breakpoint::BreakpointError> {
        unsafe { self.target.register_sw_breakpoint(addr, thumb, false) }
    }

    unsafe fn handle_debug_event(&mut self, ctx: &mut DebugEventContext) {
        self.target.set_breakpoints_ignored(true);

        let was_locked = self.target.hw_manager.locked();
        self.target.hw_manager.set_locked(false);
        self.target.exception_ctx = ctx.clone();

        let reason = self.target.hw_manager.last_break_reason();

        let bkpt_address = self.target.exception_ctx.program_counter;
        let tracked_bkpt_id = self.target.query_sw_breakpoint(bkpt_address);

        let is_manual_bkpt =
            tracked_bkpt_id.is_none() && reason == Some(DebugEventReason::BkptInstr);

        // If we previously wanted to single step, we can permanently remove the breakpoint that
        // supported that now. The saved single step request isn't removed yet so that the stop
        // reason we report to GDB is correct.
        if let Some(single_step) = self.target.single_step_request {
            self.target.hw_manager.remove_breakpoint_at(
                single_step.target_addr,
                Specificity::Mismatch,
                single_step.kind,
            );
        }

        if is_manual_bkpt {
            // Normally we try to avoid an infinite loop of breakpoints by replacing tracked
            // software breakpoints with their real instructions and re-running them. But if the
            // `bkpt` *is* the real instruction then we don't need to do the normal
            // replace-and-rerun thing. Instead, we just skip over it because its side-effect has
            // been completed.

            // SAFETY: Since the address was able to be properly fetched, it implies it is valid for
            // reads.
            let instr = unsafe { self.target.exception_ctx.read_instr() };
            self.target.exception_ctx.program_counter += instr.size() as u32;
        }

        let mut show_debug_console = true;

        if let Some(id) = tracked_bkpt_id
            && let Some(bkpt) = self.target.breaks[id]
        {
            // Some tracked breakpoints weren't requested by the user and are just used internally.
            // These should be transparent to the user by default. Note: It's possible
            // for a breakpoint to be both requested by the user and used internally.
            show_debug_console = bkpt.reason.user;

            // If this breakpoint is used internally, run any necessary callbacks.
            if bkpt.reason.internal {
                show_debug_console |= self.handle_internal_breakpoint();
            }
        }

        if show_debug_console {
            self.run_debug_console();
        }

        // Write any modifications back to the stack so the assembly code restores the updated state
        *ctx = self.target.exception_ctx.clone();

        self.target.hw_manager.set_locked(was_locked);
        self.target.set_breakpoints_ignored(false);
    }
}
