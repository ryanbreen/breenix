#![no_std] // don't link the Rust standard library
#![no_main] // disable all Rust-level entry points

#![feature(ptr_internals, abi_x86_interrupt, const_fn, custom_test_frameworks)]

#![test_runner(breenix::test_runner)]
#![reexport_test_harness_main = "test_main"]

use core::panic::PanicInfo;

pub mod constants;
pub mod event;
pub mod io;
pub mod interrupts;
pub mod util;

/// This function is called on panic.
#[cfg(not(test))]
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    println!("{}", info);
    breenix::hlt_loop();
}

#[cfg(test)]
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    breenix::test_panic_handler(info)
}

#[test_case]
fn trivial_assertion() {
    assert_eq!(1, 1);
}

#[no_mangle]
pub extern "C" fn _start() {

    println!("We're back{}", "!");

    io::initialize();
    interrupts::initialize();

    #[cfg(test)]
    test_main();

    breenix::hlt_loop();
}
