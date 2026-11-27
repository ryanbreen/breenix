//! The driver owns a distinct, hidden Itanium C++ ABI §3.3.5 object identity.

#![no_std]

// ELF gABI §§5.1 and 5.4: both LP64 targets use one hidden 64-bit object.
#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
core::arch::global_asm!(
    ".pushsection .data.rel.ro.breenix_dso_handle,\"aw\"",
    ".p2align 3",
    ".globl __dso_handle",
    ".hidden __dso_handle",
    ".type __dso_handle, %object",
    "__dso_handle:",
    ".quad __dso_handle",
    ".size __dso_handle, .-__dso_handle",
    ".popsection",
);
