//! true - return successful exit status
//!
//! Usage: true
//!
//! Exit with a status code indicating success (0).
//! This command does nothing and always succeeds.

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libbreenix::process::exit;

#[no_mangle]
pub extern "C" fn _start() -> ! {
    exit(0);
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    exit(0); // Even on panic, true returns success
}
