//! false - return unsuccessful exit status
//!
//! Usage: false
//!
//! Exit with a status code indicating failure (1).
//! This command does nothing and always fails.

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libbreenix::process::exit;

#[no_mangle]
pub extern "C" fn _start() -> ! {
    exit(1);
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    exit(1);
}
