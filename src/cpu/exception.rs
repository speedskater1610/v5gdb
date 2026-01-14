#[must_use]
pub fn vbar() -> u32 {
    let vbar: u32;
    unsafe {
        core::arch::asm!(
            "mrc p15, 0, {0}, c12, c0, 0",
            out(reg) vbar,
            options(nostack, preserves_flags)
        );
    }
    vbar
}

/// The status of an ARMv7 CPU.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(transparent)]
pub struct ProgramStatus(pub u32);

impl ProgramStatus {
    #[must_use]
    pub const fn to_raw(self) -> u32 {
        self.0
    }

    /// Returns whether the CPU state has a 16-bit instruction set enabled (Thumb or ThumbEE).
    #[must_use]
    pub const fn is_thumb(self) -> bool {
        const T_BIT: u32 = 1 << 5;
        self.0 & T_BIT != 0
    }
}
