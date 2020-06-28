#![no_std] // don't link the Rust standard library
#![no_main] // disable all Rust-level entry points

#![feature(ptr_internals, abi_x86_interrupt, const_fn, custom_test_frameworks)]

#![test_runner(breenix::test_runner)]
#![reexport_test_harness_main = "test_main"]

extern crate alloc;

use bootloader::{BootInfo, entry_point};

use alloc::{boxed::Box, vec, vec::Vec, rc::Rc};
use core::panic::PanicInfo;

pub mod constants;
pub mod event;
pub mod io;
pub mod interrupts;
pub mod state;
pub mod util;

pub use breenix::hlt_loop;

/// This function is called on panic.
#[cfg(not(test))]
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    println!("{}", info);
    hlt_loop();
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

entry_point!(kernel_main);

pub fn kernel_main(boot_info: &'static BootInfo) -> ! {
    
    println!("We're back!");

    use x86_64::{structures::paging::Page, VirtAddr};

    let phys_mem_offset = VirtAddr::new(boot_info.physical_memory_offset);

    let mut mapper = unsafe { breenix::memory::init(phys_mem_offset) };

    let mut frame_allocator = unsafe {
        breenix::memory::BootInfoFrameAllocator::init(&boot_info.memory_map)
    };

    breenix::memory::allocator::init_heap(&mut mapper, &mut frame_allocator)
        .expect("heap initialization failed");

    // allocate a number on the heap
    let heap_value = Box::new(41);
    println!("heap_value at {:p}", heap_value);

    // create a dynamically sized vector
    let mut vec = Vec::new();
    for i in 0..500 {
        vec.push(i);
    }
    println!("vec at {:p}", vec.as_slice());

    // create a reference counted vector -> will be freed when count reaches 0
    let reference_counted = Rc::new(vec![1, 2, 3]);
    let cloned_reference = reference_counted.clone();
    println!("current reference count is {}", Rc::strong_count(&cloned_reference));
    core::mem::drop(reference_counted);
    println!("reference count is {} now", Rc::strong_count(&cloned_reference));

    io::initialize();
    interrupts::initialize();

    #[cfg(test)]
    test_main();

    state::debug();

    hlt_loop();
}
