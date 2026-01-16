.arm
.text

@ A position-independent function that can be written on top of existing functions to make them
@ jump to `target_addr` instead. This is used to automatically add packet framing to user serial
@ writes when a debugger is active. For more information, see the `sdk_overlay` module.
v5gdb_sdk_trampoline:
    ldr r12, .Ltarget_addr
    bx r12
v5gdb_sdk_trampoline_end:
    @ When this function is copied to another location, this should be set to the trampoline's
    @ destination.
    .Ltarget_addr: .word 0
