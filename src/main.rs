#![no_std] // don't link the Rust standard library
#![no_main] // disable all Rust-level entry points

#![feature(ptr_internals, const_fn)]

use core::panic::PanicInfo;

pub mod constants;
pub mod io;

/// This function is called on panic.
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {}
}

#[no_mangle]
pub extern "C" fn _start() {
    println!("We're back{}", "!");

    loop {}
}