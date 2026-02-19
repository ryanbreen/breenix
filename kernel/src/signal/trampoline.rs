//! Signal trampoline for returning from signal handlers
//!
//! The trampoline calls rt_sigreturn to restore the pre-signal context.
//! This code is written to the user stack when delivering a signal, and
//! the signal handler returns to it.

// =============================================================================
// x86_64 Signal Trampoline
// =============================================================================

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
#[cfg(target_arch = "x86_64")]
pub static SIGNAL_TRAMPOLINE: [u8; 11] = [
    0x48, 0xC7, 0xC0, 0x0F, 0x00, 0x00, 0x00, // mov rax, 15 (rt_sigreturn)
    0xCD, 0x80, // int 0x80
    0x0F, 0x0B, // ud2 (should never reach here)
];

/// Size of the signal trampoline in bytes (x86_64)
#[cfg(target_arch = "x86_64")]
pub const SIGNAL_TRAMPOLINE_SIZE: usize = SIGNAL_TRAMPOLINE.len();

// =============================================================================
// ARM64 Signal Trampoline
// =============================================================================

/// The signal trampoline code (ARM64)
/// This is the raw machine code that will be executed in userspace.
///
/// Assembly (little-endian ARM64):
///   mov x8, #139     ; SYS_rt_sigreturn (aarch64 syscall number 139)
///   svc #0           ; Trigger syscall
///   brk #1           ; Should never reach here (causes debug exception if it does)
///
/// On ARM64, the signal handler returns via BLR/RET to x30 (link register),
/// which we set to point to this trampoline.
///
/// Note: ARM64 uses asm-generic syscall numbers, NOT x86_64 numbers.
/// rt_sigreturn is 139 on ARM64, not 15 as on x86_64.
///
/// Instruction encoding (little-endian):
/// - mov x8, #139:  0xD2801168 -> 68 11 80 D2
/// - svc #0:        0xD4000001 -> 01 00 00 D4
/// - brk #1:        0xD4200020 -> 20 00 20 D4
#[cfg(target_arch = "aarch64")]
pub static SIGNAL_TRAMPOLINE: [u8; 12] = [
    0x68, 0x11, 0x80, 0xD2, // mov x8, #139 (rt_sigreturn - aarch64 syscall number)
    0x01, 0x00, 0x00, 0xD4, // svc #0 (supervisor call - trigger syscall)
    0x20, 0x00, 0x20, 0xD4, // brk #1 (should never reach here)
];

/// Size of the signal trampoline in bytes (ARM64)
#[cfg(target_arch = "aarch64")]
pub const SIGNAL_TRAMPOLINE_SIZE: usize = SIGNAL_TRAMPOLINE.len();
