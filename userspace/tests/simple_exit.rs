//! Simple exit test - just exits with code 42, no int3
//!
//! This is used to test exec from ext2 without breakpoint handling.

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libbreenix::process;

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // Just exit with code 42 - no int3, no printing
    process::exit(42);
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    process::exit(255);
}
