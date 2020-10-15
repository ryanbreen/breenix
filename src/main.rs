#![no_std] // don't link the Rust standard library
#![no_main] // disable all Rust-level entry points

#![feature(alloc_error_handler, ptr_internals, abi_x86_interrupt, const_fn, custom_test_frameworks, wake_trait)]

#![test_runner(breenix::test_runner)]
#![reexport_test_harness_main = "test_main"]

extern crate alloc;
extern crate num_traits;

use bootloader::{BootInfo, entry_point};

use alloc::{boxed::Box, vec, vec::Vec, rc::Rc};
use core::panic::PanicInfo;

pub mod constants;
pub mod event;
pub mod io;
pub mod interrupts;
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

    let phys_mem_offset = VirtAddr::new(boot_info.physical_memory_offset);

    let mut mapper = unsafe { memory::init(phys_mem_offset) };

    println!("{:?}", &boot_info.memory_map);

    let mut frame_allocator = unsafe {
        memory::BootInfoFrameAllocator::init(&boot_info.memory_map)
    };

    memory::allocator::init_heap(&mut mapper, &mut frame_allocator)
        .expect("heap initialization failed");

    interrupts::initialize();
    io::initialize();

    use task::{Task, executor::Executor};
    let mut executor = Executor::new();
    executor.spawn(Task::new(io::keyboard::read()));
    executor.run();

    #[cfg(test)]
    test_main();
}
