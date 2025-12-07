//! Signal trampoline for returning from signal handlers
//!
//! The trampoline calls rt_sigreturn to restore the pre-signal context.
//! This code is written to the user stack when delivering a signal, and
//! the signal handler returns to it.

/// The signal trampoline code (x86_64)
/// This is the raw machine code that will be executed in userspace.
///
/// Assembly:
///   mov rax, 15      ; SYS_rt_sigreturn (syscall number 15)
///   int 0x80         ; Trigger syscall
///   ud2              ; Should never reach here (causes illegal instruction if it does)
///
/// The handler's `ret` instruction will pop the return address and jump to
/// this trampoline code. The trampoline then invokes sigreturn to restore
/// the saved context.
pub static SIGNAL_TRAMPOLINE: [u8; 11] = [
    0x48, 0xC7, 0xC0, 0x0F, 0x00, 0x00, 0x00, // mov rax, 15 (rt_sigreturn)
    0xCD, 0x80, // int 0x80
    0x0F, 0x0B, // ud2 (should never reach here)
];

/// Size of the signal trampoline in bytes
pub const SIGNAL_TRAMPOLINE_SIZE: usize = SIGNAL_TRAMPOLINE.len();
