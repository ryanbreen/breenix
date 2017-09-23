#![feature(alloc, allocator, box_syntax, box_patterns, macro_reexport, lang_items,
          heap_api, const_fn, unique, asm, collections, trace_macros, const_unsafe_cell_new,
          naked_functions, drop_types_in_const, stmt_expr_attributes, core_intrinsics, abi_x86_interrupt)]
#![allocator]

#![no_std]

// Note: We must define macros first since it declared printk!  Otherwise, no
// other mod can print.
#[macro_use]
mod macros;

extern crate tiered_allocator;

extern crate libbreenix;

#[macro_use]
extern crate collections;
extern crate alloc;
extern crate rlibc;

#[macro_use]
extern crate spin;

#[macro_use]
extern crate once;

extern crate multiboot2;

extern crate cpuio;

extern crate bit_field;

extern crate volatile;

#[macro_use]
extern crate bitflags;

#[macro_use]
extern crate lazy_static;

#[macro_use(int)]
extern crate x86;

extern crate x86_64;

#[macro_use]
mod util;

mod constants;
mod debug;
mod event;
mod memory;
//mod vga_writer;

mod interrupts;
mod io;

mod state;
mod task;

static mut MEMORY_SAFE:bool = false;

#[no_mangle]
#[allow(non_snake_case)]
pub fn _Unwind_Resume() {
    printk!("UNWIND!");
    state().scheduler.idle();
}

#[lang = "panic_fmt"]
#[no_mangle]
pub extern "C" fn panic_fmt(fmt: core::fmt::Arguments, file: &str, line: u32) -> ! {
    printk!("\n\nPANIC in {} at line {}:", file, line);
    printk!("    {}", fmt);
    loop {}
}

fn enable_nxe_bit() {
    use x86::shared::msr::{IA32_EFER, rdmsr, wrmsr};

    let nxe_bit = 1 << 11;
    unsafe {
        let efer = rdmsr(IA32_EFER);
        wrmsr(IA32_EFER, efer | nxe_bit);
    }
}

fn enable_write_protect_bit() {
    use x86::shared::control_regs::{cr0, cr0_write, CR0_WRITE_PROTECT};

    unsafe { cr0_write(cr0() | CR0_WRITE_PROTECT) };
}

#[no_mangle]
pub extern "C" fn rust_main(multiboot_information_address: usize) {

    printk!("burr check what we got");

    let boot_info = unsafe { multiboot2::load(multiboot_information_address) };

    enable_nxe_bit();
    enable_write_protect_bit();

    // set up guard page and map the heap pages
    memory::init(boot_info);

    state();
    unsafe { MEMORY_SAFE = true; }

    // Now that we have a heap, allow printk.
    io::printk::init();

    //printk!("{:?}", memory::area_frame_allocator());
    state().scheduler.create_test_process();

    interrupts::init();

    io::initialize();

    printk!("Time is {}", io::timer::real_time());

    use alloc::boxed::Box;
    use collections::Vec;
    let mut vec: Box<Vec<&'static str>> = Box::new(Vec::new());

    printk!("{} {}",
             unsafe { tiered_allocator::BOOTSTRAP_ALLOCS },
             unsafe { tiered_allocator::BOOTSTRAP_ALLOC_SIZE });

    let my_str = "happy days are here";

    // Fails with PF if push count > 8192 because we don't support slabs that large.
    for _ in 0..8192 {
        vec.push(my_str);
    }

    printk!("Created a vector with {} items?  Great work. {}",
             vec.len(),
             vec[127]);

    state().scheduler.enable_interrupts();

    //state().scheduler.schedule();
    printk!("idling");
    state().scheduler.idle();
}

pub fn memory_safe() -> bool {
    unsafe { MEMORY_SAFE }
}

/// Provide an easy, globally accessible function to get access to State
pub fn state() -> &'static mut state::State {
    state::state()
}

#[lang = "eh_personality"]
extern "C" fn eh_personality() {}
