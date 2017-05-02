#![feature(alloc, allocator, box_syntax, box_patterns, macro_reexport, lang_items,
          heap_api, const_fn, unique, asm, collections, trace_macros,
          naked_functions, stmt_expr_attributes, core_intrinsics, abi_x86_interrupt)]
#![allocator]

#![no_std]

// Note: We must define macros first since it declared println!  Otherwise, no
// other mod can print.
#[macro_use]
mod macros;

// extern crate hole_list_allocator;
extern crate tiered_allocator;

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

mod writers;
mod constants;
mod debug;
mod event;
mod memory;
//mod vga_writer;

mod interrupts;
mod io;

mod state;
mod task;

#[no_mangle]
#[allow(non_snake_case)]
pub fn _Unwind_Resume() {
    println!("UNWIND!");
    state().scheduler.idle();
}

#[lang = "panic_fmt"]
#[no_mangle]
pub extern "C" fn panic_fmt(fmt: core::fmt::Arguments, file: &str, line: u32) -> ! {
//    println!("\n\nPANIC in {} at line {}:", file, line);
//    println!("    {}", fmt);
    state().scheduler.idle();
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

    let boot_info = unsafe { multiboot2::load(multiboot_information_address) };

    enable_nxe_bit();
    enable_write_protect_bit();

    // set up guard page and map the heap pages
    let mut memory_controller = memory::init(boot_info);

    // Phil-Opp idt
    interrupts::init(&mut memory_controller);
    
    x86_64::instructions::interrupts::int3();

    fn stack_overflow() {
        stack_overflow(); // for each recursion, the return address is pushed
    }

    // trigger a stack overflow
    //stack_overflow();
    io::initialize();

    // provoke a page fault
    //unsafe { *(0xdeadbeaf as *mut u64) = 42 };

    println!("It did not crash");

    println!("Time is {}", io::timer::real_time().secs);

    println!("{:?}", memory::area_frame_allocator());

    use alloc::boxed::Box;
    use collections::Vec;
    let mut vec: Box<Vec<&'static str>> = Box::new(Vec::new());

    println!("{} {}",
             unsafe { tiered_allocator::BOOTSTRAP_ALLOCS },
             unsafe { tiered_allocator::BOOTSTRAP_ALLOC_SIZE });

    let my_str = "happy days are here";
    // Fails with PF if push count > 8192 because we don't support slabs that large.
    for _ in 0..8192 {
        vec.push(my_str);
    }

    println!("Created a vector with {} items?  Great work. {}",
             vec.len(),
             vec[127]);

    debug!();

    // state().scheduler.schedule();
    println!("idling");
    state().scheduler.idle();
}

/// Provide an easy, globally accessible function to get access to State
pub fn state() -> &'static mut state::State {
    state::state()
}

#[lang = "eh_personality"]
extern "C" fn eh_personality() {}
