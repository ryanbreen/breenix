//! The driver owns a distinct, hidden Itanium ABI object identity.

#![no_std]

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
