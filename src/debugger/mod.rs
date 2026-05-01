//! Main debugger loop and event handling logic.

use core::{
    convert::Infallible,
    sync::atomic::{AtomicBool, Ordering},
};

use gdbstub::{
    conn::{Connection, ConnectionExt},
    stub::{
        GdbStub, GdbStubBuilder, GdbStubError, MultiThreadStopReason, SingleThreadStopReason,
        state_machine::GdbStubStateMachine,
    },
};
use snafu::Snafu;
use spin::{Mutex, MutexGuard, Once};
use zynq7000::devcfg;

use crate::{
    Debugger,
    cpu::debug::DebugEventReason,
    debugger::sdk::InternalBreakpoint,
    exceptions::DebugEventContext,
    gdb_target::{V5Target, breakpoint::hardware::Specificity},
    sdk::stop_all_motors,
    sys::{DebuggerSystem, System},
    transport::TransportError,
};

pub mod sdk;

#[derive(Debug, Snafu)]
pub enum DebuggerError {
    #[snafu(context(false))]
    Io { source: TransportError },
    GdbStub {
        inner: GdbStubError<Infallible, TransportError>,
    },
}

impl From<GdbStubError<Infallible, TransportError>> for DebuggerError {
    fn from(value: GdbStubError<Infallible, TransportError>) -> Self {
        Self::GdbStub { inner: value }
    }
}

/// Initial configuration for [`V5Debugger`].
///
/// Stores the debugger's default settings. These values are applied once during
/// [`Debugger::initialize`] and have no effect if modified afterwards. Use GDB monitor commands
/// to change settings at runtime once the debugger is running.
#[derive(Debug, Default, Clone)]
pub struct DebuggerConfig {
    /// Whether all motors should be stopped by default when a breakpoint fires.
    ///
    /// When `true`, [`sdk::stop_all_motors`] is called immediately on every breakpoint, before
    /// the GDB console loop begins. This prevents the robot from driving away or actuating
    /// mechanisms while execution is paused.
    ///
    /// This is the *initial* value of [`V5Target::stop_motors_on_break`]. It can be overridden
    /// at runtime from GDB with `monitor autostop true` / `monitor autostop false` without
    /// restarting the program.
    ///
    /// Defaults to `false`.
    pub stop_motors_on_break: bool,
}

/// Debugger manager.
pub struct V5Debugger<S>
where
    S: Connection<Error = TransportError> + ConnectionExt,
{
    state: Mutex<DebuggerState<'static, S>>,
    /// Initial settings applied to the debugger on [`Debugger::initialize`].
    ///
    /// After initialisation, the live values in [`V5Target`] are the source of truth and this
    /// field is no longer read. Mutating it after calling [`install`](crate::install) has no
    /// effect.
    config: DebuggerConfig,
}

impl<S> V5Debugger<S>
where
    S: Connection<Error = TransportError> + ConnectionExt,
{
    /// creates a new debugger with default config.
    ///
    /// By default, motors are **not** stopped on breakpoints. Pass a [`DebuggerConfig`] via
    /// [`with_config`](Self::with_config), or use the convenience builder
    /// [`with_motor_stop`](Self::with_motor_stop). Settings can also be changed at runtime from
    /// GDB using monitor commands.
    #[must_use]
    pub fn new(stream: S) -> Self {
        const GDB_PACKET_BUFFER_SIZE: usize = 4096;
        static mut GDB_PACKET_BUFFER: [u8; GDB_PACKET_BUFFER_SIZE] = [0; _];
        static GDB_PACKET_BUFFER_CLAIMED: AtomicBool = AtomicBool::new(false);

        if GDB_PACKET_BUFFER_CLAIMED.swap(true, Ordering::Acquire) {
            panic!("Cannot create multiple debuggers");
        }

        // SAFETY: The mutable ownership over the buffer can only be taken once.
        let gdb_buffer = unsafe {
            core::slice::from_raw_parts_mut(&raw mut GDB_PACKET_BUFFER[0], GDB_PACKET_BUFFER_SIZE)
        };

        let target = V5Target::new(&mut unsafe { devcfg::Registers::new_mmio_fixed() });

        Self {
            state: Mutex::new(DebuggerState {
                gdb: {
                    Some(
                        GdbStubBuilder::new(stream)
                            .with_packet_buffer(gdb_buffer)
                            .build()
                            .unwrap(),
                    )
                },
                stub: None,
                target,
                internal_breaks: None,
            }),
            config: DebuggerConfig::default(),
        }
    }

    /// applies a [`DebuggerConfig`] to this debugger.
    ///
    /// replaces any previously set configuration. Has no effect if called after
    /// [`install`](crate::install).
    #[must_use]
    pub fn with_config(mut self, config: DebuggerConfig) -> Self {
        self.config = config;
        self
    }

    /// sets whether all motors should be automatically stopped whenever a breakpoint fires.
    ///
    /// this sets [`DebuggerConfig::stop_motors_on_break`] and controls the *default* value of
    /// `V5Target::stop_motors_on_break`. it is applied once at initialisation and has no effect
    /// if changed after [`install`](crate::install) is called
    ///
    /// use `monitor autostop true` / `monitor autostop false` from GDB to toggle the setting
    /// at runtime without restarting the program
    ///
    /// Defaults to `false`.
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use v5gdb::{debugger::V5Debugger, transport::StdioTransport};
    ///
    /// v5gdb::install(
    ///     V5Debugger::new(StdioTransport)
    ///         .with_motor_stop(true),
    /// );
    /// ```
    #[must_use]
    pub fn with_motor_stop(mut self, enabled: bool) -> Self {
        self.config.stop_motors_on_break = enabled;
        self
    }

    /// Returns the debugger's internal state.
    #[must_use]
    pub fn state<'a>(&'a self) -> MutexGuard<'a, DebuggerState<'static, S>> {
        self.state.lock()
    }
}

unsafe impl<S> Debugger for V5Debugger<S>
where
    S: Connection<Error = TransportError> + ConnectionExt + Send + 'static,
{
    fn initialize(&self) {
        let mut state = self.state();
        state.register_internal_breakpoints();
        System::initialize(&mut state.target);
        crate::sdk::competition::install_override();

        // apply the initial config into the live target state. from this point on, the values in
        // V5Target are the source of truth so `self.config` is never read again.
        state.target.stop_motors_on_break = self.config.stop_motors_on_break;

        log::debug!("Debugger initialized (config={:?})", self.config);
    }

    unsafe fn handle_debug_event(&self, ctx: &mut DebugEventContext) -> bool {
        let mut state = self.state();
        // Pause software breakpoints before allowing unpredictable control flow (by interrupts).
        state.target.set_breakpoints_ignored(true);

        // We re-enable interrupts after the abort (so that UART works) but prevent the RTOS from
        // preempting us. When the debugger is active, the system should appear paused.

        // If we're handling a single-step completion, the scheduler is already disabled from when
        // the step was initiated (previous debug session), so there's no need to do that again.
        if state.target.single_step_request.is_none() {
            System::suspend_preemption();
        }
        unsafe {
            aarch32_cpu::interrupt::enable();
        }

        log::debug!("Entered debug event handler");
        static BKPT_LOG: Once = Once::new();
        BKPT_LOG.call_once(|| {
            log::error!("**** v5gdb: BREAKPOINT TRIGGERED ****");
            log::error!("Your program has been paused. Please connect a debugger.")
        });

        // Auto motor-stop on breakpoint.
        //
        // We read from `target.stop_motors_on_break` (not `self.config.stop_motors_on_break`)
        // so that live changes via `monitor autostop true/false` take effect on the next
        // breakpoint without restarting the program.
        if state.target.stop_motors_on_break {
            log::debug!("Auto motor-stop triggered by breakpoint");
            stop_all_motors();
        }

        let was_locked = state.target.hw_manager.locked();
        state.target.hw_manager.set_locked(false);
        state.target.exception_ctx = ctx.clone();

        let reason = state.target.hw_manager.last_break_reason();

        let bkpt_address = state.target.exception_ctx.program_counter;
        let tracked_bkpt_id = state.target.query_sw_breakpoint(bkpt_address);

        state.target.last_stop_was_hardcoded =
            tracked_bkpt_id.is_none() && reason == Some(DebugEventReason::BkptInstr);

        // If we previously wanted to single step, we can permanently remove the breakpoint that
        // supported that now. The single step request is then cleared since we've finished all
        // required cleanup.
        if let Some(single_step) = state.target.single_step_request.take() {
            state.target.hw_manager.remove_breakpoint_at(
                single_step.target_addr,
                Specificity::Mismatch,
                single_step.kind,
            );
        }

        if state.target.last_stop_was_hardcoded {
            // Normally we try to avoid an infinite loop of breakpoints by replacing tracked
            // software breakpoints with their real instructions and re-running them. But if the
            // `bkpt` *is* the real instruction then we don't need to do the normal
            // replace-and-rerun thing. Instead, we just skip over it because its side-effect has
            // been completed.

            // SAFETY: Since the address was able to be properly fetched, it implies it is valid for
            // reads.
            let instr = unsafe { state.target.exception_ctx.read_instr() };
            state.target.exception_ctx.program_counter += instr.size() as u32;
        }

        let mut show_debug_console = true;

        if let Some(id) = tracked_bkpt_id
            && let Some(bkpt) = state.target.breaks[id]
        {
            // Some tracked breakpoints weren't requested by the user and are just used internally.
            // These should be transparent to the user by default. Note: It's possible
            // for a breakpoint to be both requested by the user and used internally.
            show_debug_console = bkpt.reason.user;

            // If this breakpoint is used internally, run any necessary callbacks.
            if bkpt.reason.internal {
                show_debug_console |= state.handle_internal_breakpoint();
            }
        }

        if show_debug_console {
            log::debug!("Starting debug console");
            state.run_debug_console();
            log::debug!("Debug console has exited");
        }

        // Write any modifications back to the stack so the assembly code restores the updated state
        *ctx = state.target.exception_ctx.clone();

        log::debug!("Exiting debug event handler");

        // Single steps run with the scheduler off so that we are guaranteed to step the current
        // task, not a different one. - Side note: If PROS implemented ARM's context id register, we
        // could just filter the single step breakpoint by task id and there would be no need for
        // this.
        let should_unpause_scheduler = state.target.single_step_request.is_none();

        state.target.hw_manager.set_locked(was_locked);
        state.target.set_breakpoints_ignored(false);

        should_unpause_scheduler
    }
}

/// Internal mutable state of debugger.
pub struct DebuggerState<'a, S>
where
    S: Connection<Error = TransportError> + ConnectionExt,
{
    pub target: V5Target,
    internal_breaks: Option<[(InternalBreakpoint, u32); 1]>,
    gdb: Option<GdbStub<'a, V5Target, S>>,
    stub: Option<GdbStubStateMachine<'a, V5Target, S>>,
}

impl<S> DebuggerState<'_, S>
where
    S: Connection<Error = TransportError> + ConnectionExt,
{
    fn has_client(&self) -> bool {
        let disconnected = matches!(
            &self.stub,
            None | Some(GdbStubStateMachine::Disconnected(_))
        );
        !disconnected
    }

    /// Runs the debug console until the user indicates they want to continue program execution.
    fn run_debug_console(&mut self) {
        if let Some(gdb) = self.gdb.take() {
            // Initial GDB setup - calls connection setup callback.
            self.stub = Some(gdb.run_state_machine(&mut self.target).unwrap());
        }
        let mut gdb = self.stub.take().unwrap();

        // Enter debugging loop until it's time to resume.

        self.target.reset_resume();
        while !self.target.resume {
            unsafe {
                vex_sdk::vexTasksRun();
            }

            gdb = Self::tick_state_machine(gdb, &mut self.target)
                .expect("debugger encountered an error");
        }

        self.target.resume = false;
        self.stub = Some(gdb);
    }

    fn tick_state_machine<'a>(
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
                log::info!("Debugger Stop reason: {reported_reason:?}");

                // Once we tell GDB we've exited we should exit the monitor because the session will
                // end.
                if matches!(reported_reason, MultiThreadStopReason::Exited(_)) {
                    target.resume = true;
                }

                Ok(gdb.report_stop(target, reported_reason)?)
            }
            GdbStubStateMachine::CtrlCInterrupt(gdb) => {
                log::warn!("Got Ctrl+C");
                let stop_reason: Option<SingleThreadStopReason<_>> = None;
                Ok(gdb.interrupt_handled(target, stop_reason)?)
            }
            GdbStubStateMachine::Disconnected(gdb) => Ok(gdb.return_to_idle()),
        }
    }
}
