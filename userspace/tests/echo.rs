//! echo - print arguments to stdout
//!
//! Usage: echo [STRING]...
//!
//! Prints a newline. When argv support is added, will print arguments.

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libbreenix::io::{println, stderr};
use libbreenix::process::exit;

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // Without argv support, echo just prints a newline
    // This mimics `echo` with no arguments
    println("");
    exit(0);
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    let _ = stderr().write_str("echo: panic!\n");
    exit(2);
}
