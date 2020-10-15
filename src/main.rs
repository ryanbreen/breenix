#![no_std] // don't link the Rust standard library
#![no_main] // disable all Rust-level entry points
#![feature(
    alloc_error_handler,
    ptr_internals,
    abi_x86_interrupt,
    const_fn,
    custom_test_frameworks,
    wake_trait
)]
#![test_runner(breenix::test_runner)]
#![reexport_test_harness_main = "test_main"]

extern crate alloc;
extern crate num_traits;

use bootloader::{entry_point, BootInfo};

use alloc::{boxed::Box, rc::Rc, vec, vec::Vec};
use core::panic::PanicInfo;

pub mod constants;
pub mod event;
pub mod interrupts;
pub mod io;
pub mod memory;
pub mod state;
pub mod task;
pub mod util;

#[macro_export]
pub mod macros;

pub fn hlt_loop() -> ! {
    loop {
        x86_64::instructions::hlt();
    }
}

#[alloc_error_handler]
fn alloc_error_handler(layout: alloc::alloc::Layout) -> ! {
    panic!("allocation error: {:?}", layout)
}

/// This function is called on panic.
#[cfg(not(test))]
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    println!("{}", info);
    hlt_loop();
}

entry_point!(kernel_main);

pub fn kernel_main(boot_info: &'static BootInfo) -> ! {
    println!("We're back!");

    use x86_64::{structures::paging::Page, VirtAddr};

    memory::init(&boot_info);

    interrupts::initialize();
    io::initialize();

    use task::{executor::Executor, Task};
    let mut executor = Executor::new();
    executor.spawn(Task::new(io::keyboard::read()));
    executor.run();

    #[cfg(test)]
    test_main();
}
