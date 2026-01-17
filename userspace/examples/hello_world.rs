//! Hello World userspace test
//!
//! This is the simplest userspace test - it triggers an int3 breakpoint to prove
//! Ring 3 execution, then exits.

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libbreenix::io;
use libbreenix::process;

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // CRITICAL: int3 as the absolute first instruction to prove CPL3 execution
    // This breakpoint is caught by the kernel to verify Ring 3 is working
    unsafe {
        core::arch::asm!("int3", options(nomem, nostack));
    }

    // Exit cleanly with code 42
    process::exit(42);
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    io::print("Second process panic!\n");
    process::exit(2);
}
