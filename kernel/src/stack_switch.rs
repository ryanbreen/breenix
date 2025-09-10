//! Stack switching trampoline for migrating from bootstrap stack to kernel stack
//! 
//! This module provides a safe way to switch from the bootstrap stack (PML4[3])
//! to the proper upper-half kernel stack (PML4[402]) during early boot.

use core::ffi::c_void;

/// Switch to a new stack and call a continuation function with one argument
/// 
/// # Safety
/// 
/// This function switches the stack pointer and never returns.
/// The continuation function must also never return.
/// Interrupts must be disabled when calling this.
/// 
/// Uses x86_64 SysV ABI:
/// - rdi = stack_top (first argument)
/// - rsi = entry function pointer (second argument)  
/// - rdx = arg to pass to entry (third argument)
#[unsafe(naked)]
pub unsafe extern "C" fn switch_stack_and_call_with_arg(
    _stack_top: u64,
    _entry: extern "C" fn(*mut c_void) -> !,
    _arg: *mut c_void,
) -> ! {
    core::arch::naked_asm!(
        // rdi = stack_top, rsi = entry, rdx = arg (SysV ABI)
        "mov rsp, rdi",      // switch to new stack
        "and rsp, -16",      // ensure 16-byte alignment before call
        "mov rdi, rdx",      // move arg into first-arg reg
        "call rsi",          // call entry(arg) — must not return
        "ud2"                // trap if it does
    );
}