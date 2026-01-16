//! Automatic framing of user serial writes.
//!
//! This module contains alternative implementations of certain VEX SDK functions which capture
//! calls to functions like `vexSerial*` to automatically add framing via a multiplexing protocol.
//!
//! The current implementation is compatible with the following SDKs:
//!
//! - vex_sdk_jumptable
//! - v5rt
//! - v5rts

#![allow(non_snake_case)]

use core::{arch::global_asm, ptr};

use cobs::CobsEncoder;
use cortex_ar::asm::{dsb, isb};

use crate::cpu::cache::{self, CacheTarget};

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
unsafe fn redirect_function(target: *mut u32, destination: *const u32) {
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

/// Capture calls to `vexSerial*` functions and automatically add multiplexing packet framing,
/// sending them over the User channel.
pub fn enable_auto_muxing() {
    unsafe {
        redirect_function(
            vex_sdk::vexSerialWriteBuffer as *mut u32,
            user_write_buffer as *const u32,
        );

        redirect_function(
            vex_sdk::vexSerialWriteChar as *mut u32,
            user_write_char as *const u32,
        );
    }
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ChannelId {
    /// Standard I/O.
    User = b'u',
    /// Debug channel. Messages should use the GDB Protocol.
    Debug = b'd',
}

pub const USER: u32 = 1;
pub const OUT_BUFFER_SIZE: usize = 2048;

/// Write one or more COBS-encoded packets to serial output, each prefixed with the given channel
/// id.
///
/// Returns the number of bytes that were written from `buf`.
pub fn write_all(channel: ChannelId, mut buf: &[u8]) {
    while !buf.is_empty() {
        let mut out_buf = [0u8; OUT_BUFFER_SIZE];

        // The actual out-buffer has 1 extra byte for the packet delimiter.
        let max_len = out_buf.len() - 1;
        let out_buf_without_delimiter = &mut out_buf[..max_len];

        let mut encoder = CobsEncoder::new(out_buf_without_delimiter);

        encoder.push(&[channel as u8]).unwrap();

        // Put as many bytes as possible into this packet.
        while let Some(&byte) = buf.first() {
            let Ok(_) = encoder.push(&[byte]) else {
                break;
            };
            buf = &buf[1..];
        }

        let length = encoder.finalize();

        write_raw(&out_buf[..=length]); // Include `0` packet delimiter.
    }
}

/// Writes up to [`OUT_BUFFER_SIZE`] bytes to serial.
fn write_raw(buf: &[u8]) {
    let vexSerialWriteBuffer =
        unsafe { jumptable!(0x89c, unsafe extern "C" fn(u32, *const u8, u32) -> i32) };

    if unsafe { vex_sdk::vexSerialWriteFree(USER) as usize } < buf.len() {
        flush_serial();
    }

    unsafe {
        vexSerialWriteBuffer(USER, buf.as_ptr(), buf.len() as u32);
    }
}

pub fn flush_serial() {
    unsafe {
        while (vex_sdk::vexSerialWriteFree(USER) as usize) != OUT_BUFFER_SIZE {
            vex_sdk::vexTasksRun();
        }
    }
}

unsafe extern "C" fn user_write_buffer(channel: u32, data: *const u8, data_len: u32) -> i32 {
    if channel != 1 {
        return -1;
    }
    let user_data = unsafe { core::slice::from_raw_parts(data, data_len as usize) };
    write_all(ChannelId::User, user_data);
    data_len as i32
}

unsafe extern "C" fn user_write_char(channel: u32, c: u8) -> i32 {
    if channel != 1 {
        return -1;
    }
    write_all(ChannelId::User, &[c]);
    1
}
