use core::fmt::Debug;

use gdbstub::conn::{Connection, ConnectionExt};
use vex_sdk::{vexSerialWriteChar, vexSerialWriteFree, vexTasksRun};

use crate::transport::mux::{ChannelId, OUT_BUFFER_SIZE};

pub mod mux;

/// A means of communicating with a debug console.
pub trait Transport: Connection<Error = std::io::Error> + ConnectionExt + Send + Clone {
    fn initialize(&mut self) {}
}

/// Debug logging via stdio.
#[derive(Debug)]
#[non_exhaustive]
pub struct StdioTransport {}

impl StdioTransport {
    /// Create a new stdio-based transport.
    #[must_use]
    pub fn new() -> Self {
        Self {}
    }
}

impl Default for StdioTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for StdioTransport {
    fn clone(&self) -> Self {
        Self::new()
    }
}

impl Transport for StdioTransport {
    fn initialize(&mut self) {
        mux::enable_auto_muxing();
    }
}

impl Connection for StdioTransport {
    type Error = std::io::Error;

    fn write(&mut self, byte: u8) -> Result<(), Self::Error> {
        mux::write_all(ChannelId::Debug, &[byte]);
        Ok(())
    }

    fn write_all(&mut self, buf: &[u8]) -> Result<(), Self::Error> {
        mux::write_all(ChannelId::Debug, buf);
        Ok(())
    }

    fn flush(&mut self) -> Result<(), Self::Error> {
        mux::flush_serial();
        Ok(())
    }
}

impl ConnectionExt for StdioTransport {
    fn peek(&mut self) -> Result<Option<u8>, Self::Error> {
        let char = unsafe { vex_sdk::vexSerialPeekChar(1) };

        if char == -1 {
            return Ok(None);
        }

        Ok(Some(char as u8))
    }

    fn read(&mut self) -> Result<u8, Self::Error> {
        loop {
            let c = unsafe { vex_sdk::vexSerialReadChar(1) };

            if c != -1 {
                return Ok(c as u8);
            }

            unsafe {
                vexTasksRun();
            }
        }
    }
}
