//! motor controls utits
//!
//! This mod provides helpers for stopping all connected motors on the
//! brain. It is used both by the `monitor stop` command (manual stop) and by
//! the auto stop-on-breakpoint feature

use vex_sdk::{V5_MAX_DEVICE_PORTS, vexDeviceGetByIndex, vexDeviceMotorVoltageSet};


/// Immediately stops every motor connected to the brain by setting its
/// voltage to 0
///
/// ports that have no device connected are silently skipped. The loop does
/// not short circuit on a null device handle so that non contiguous motor
/// configurations (e.g. motors on ports 1 and 5 with no motors on ports 2-4) can end up
/// being handled correctly.
#[cfg(target_arch = "arm")]
pub fn stop_all_motors() {
    for port_num in 0..V5_MAX_DEVICE_PORTS {
        unsafe {
            let device = vexDeviceGetByIndex(port_num as u32);
            if device.is_null() {
                // Nothing plugged in to this port -> so skip but keep going over ports
                // must not `break` here: ports can be noncontiguous; meaning a
                // null handle does not mean there are no more devices (I think maybe a spot for review)
                continue;
            }
            // setting voltage to 0 immediately cuts power to the motor
            // this works for both 11W and 5.5W motors
            vexDeviceMotorVoltageSet(device, 0);
        }
    }
}

/// No-op stub
#[cfg(not(target_arch = "arm"))]
pub fn stop_all_motors() {

}
