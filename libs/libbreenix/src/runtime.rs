//! Userspace runtime entry point for Breenix binaries.

use core::arch::naked_asm;

use crate::process::exit;

extern "C" {
    fn main(argc: usize, argv: *const *const u8) -> i32;
}

#[cfg(target_arch = "x86_64")]
#[unsafe(naked)]
#[no_mangle]
pub extern "C" fn _start() -> ! {
    naked_asm!(
        "mov rdi, rsp",    // Pass original RSP as first argument
        "and rsp, -16",    // Align stack to 16 bytes (ABI requirement)
        "call {entry}",    // Call runtime_entry(stack_ptr)
        "ud2",             // Should never return
        entry = sym runtime_entry,
    )
}

#[cfg(target_arch = "aarch64")]
#[unsafe(naked)]
#[no_mangle]
pub extern "C" fn _start() -> ! {
    naked_asm!(
        "mov x0, sp",      // Pass original SP as first argument
        "bl {entry}",      // Call runtime_entry(stack_ptr)
        "brk #1",          // Should never return
        entry = sym runtime_entry,
    )
}

extern "C" fn runtime_entry(stack_ptr: *const u64) -> ! {
    let argc = unsafe { *stack_ptr as usize };
    let argv = unsafe { stack_ptr.add(1) as *const *const u8 };
    let exit_code = unsafe { main(argc, argv) };
    exit(exit_code);
}
