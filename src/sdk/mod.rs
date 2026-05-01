//! Runtime patching of SDK functions to emulate different return values or arguments.
//!
//! This module doesn't affect the actual implementations of the underlying SDK functions, but it is
//! able to add a proxy layer between VEXos and user code via the wrapper functions defined in
//! libraries like v5rt and vex_sdk_jumptable. The wrapper functions normally get inlined into their
//! call sites when LTO is on, so the functionality in this module only works when LTO is off.

use core::{arch::global_asm, ptr};

use aarch32_cpu::asm::{dsb, isb};

use crate::cpu::cache::{self, CacheTarget};

pub mod competition;

global_asm!(include_str!("./sdk_trampoline.S"), options(raw));
unsafe extern "C" {
    /// A position-independent function that jumps to another (configurable) function.
    ///
    /// The function's body spans from the `v5gdb_sdk_trampoline` symbol until
    /// [`v5gdb_sdk_trampoline_end`]. When the trampoline routine is called, it will branch to the
    /// function pointer placed immediately after its body.
    fn v5gdb_sdk_trampoline();
    static v5gdb_sdk_trampoline_end: u32;
}

/// Overwrite the target function to branch to the given proxy when called instead of performing
/// its original functionality.
///
/// # Safety
///
/// The target function must be at least 3 words long and valid to write to. The destination
/// function must be valid to call in all the same situations as the target function and also have
/// the same signature as it.
pub unsafe fn redirect_function(target: *mut u32, destination: *const u32) {
    let trampoline_ptr = v5gdb_sdk_trampoline as *const u32;
    let trampoline_len =
        unsafe { (&raw const v5gdb_sdk_trampoline_end).offset_from_unsigned(trampoline_ptr) };
    let destination_ptr = unsafe { target.add(trampoline_len).cast() };

    unsafe {
        ptr::copy_nonoverlapping(trampoline_ptr, target, trampoline_len);
        ptr::write(destination_ptr, destination);
    }

    dsb();
    isb();

    // Sync both start and end, in case the function crosses a cache line.
    cache::sync_instruction(CacheTarget::Address(target as u32));
    cache::sync_instruction(CacheTarget::Address(destination_ptr as u32));
}

/// Directly access VEX SDK functions over the jump table without their wrappers.
///
/// This is effectively a partial re-implementation of the `vex-sdk-jumptable` crate, which we can't
/// use here because those might be the functions we are redirecting. If we were to call those
/// directly, it might cause an infinite loop.
macro_rules! jumptable {
    ($offset:literal, $ty:ty) => {{
        const JUMPTABLE_BASE: u32 = 0x037fc000;
        let ptr = (JUMPTABLE_BASE + $offset) as *const $ty;
        *ptr
    }};
}
pub(crate) use jumptable;

/// immediately stops every motor connected to the brain by setting its voltage to 0 mV.
///
/// Ports that have no device connected are silently skipped. The loop does **not** short circuit
/// on a null device handle, so noncontiguous motor configurations (e.g. motors on ports 1 and 5
/// with nothing on 2-4) are handled correctly.
///
/// # Safety
///
/// Must be called from a context where the VEX SDK is initialised and the jump-table is valid
/// (i.e. inside user-program execution on the V5 brain). This is always the case when called from
/// a breakpoint handler.
pub fn stop_all_motors() {
    use vex_sdk::{V5_MAX_DEVICE_PORTS, vexDeviceGetByIndex, vexDeviceMotorVoltageSet};
    for port_num in 0..V5_MAX_DEVICE_PORTS {
        unsafe {
            let device = vexDeviceGetByIndex(port_num as u32);
            if device.is_null() {
                // Nothing plugged in to this port skip but keep iterating.
                // Must not `break` here: ports can be non-contiguous, so a null handle does not
                // mean there are no more devices
                continue;
            }
            // setting voltage to 0mV immediately cuts power to the motor
            // this works for both 11W and 5.5W smart-motors
            vexDeviceMotorVoltageSet(device, 0);
        }
    }
}