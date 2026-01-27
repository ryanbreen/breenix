//! echo - print arguments to stdout
//!
//! Usage: echo [STRING]...
//!
//! Prints the arguments separated by spaces, followed by a newline.

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libbreenix::argv;
use libbreenix::io::{print, println, stderr, stdout};
use libbreenix::process::exit;

#[no_mangle]
pub extern "C" fn main(argc: usize, argv_ptr: *const *const u8) -> i32 {
    let args = unsafe { argv::Args::new(argc, argv_ptr) };

    // Print each argument starting from argv[1], separated by spaces
    for i in 1..args.argc {
        if let Some(arg) = args.argv(i) {
            if i > 1 {
                print(" ");
            }
            // Print the argument bytes directly
            let _ = stdout().write(arg);
        }
    }

    // Print final newline
    println("");
    0
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    let _ = stderr().write_str("echo: panic!\n");
    exit(2);
}
