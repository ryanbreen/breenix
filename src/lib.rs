#![feature(alloc, allocator, box_syntax, box_patterns, macro_reexport, lang_items, const_fn, unique, asm, collections, trace_macros, stmt_expr_attributes)]
#![allocator]

#![no_std]

// Note: We must define macros first since it declared println!  Otherwise, no
// other mod can print.
#[macro_use]
mod macros;

//extern crate hole_list_allocator;
extern crate tiered_allocator;

#[macro_use]
extern crate collections;
extern crate alloc;
extern crate rlibc;

#[macro_use]
extern crate once;

extern crate spin;

extern crate multiboot2;

extern crate cpuio;

#[macro_use]
extern crate bitflags;

#[macro_use(int)]
extern crate x86;

mod buffers;
mod constants;
mod debug;
mod event;
mod memory;
mod vga_writer;

mod io;

mod state;
mod task;
mod util;

use core::fmt::Write;

#[no_mangle]
#[allow(non_snake_case)]
pub fn _Unwind_Resume() {
  println!("UNWIND!");
  state().scheduler.idle();
}

#[lang = "panic_fmt"]
extern fn panic_fmt(fmt: core::fmt::Arguments, file: &str, line: u32) -> ! {
  println!("\n\nPANIC in {} at line {}:", file, line);
  println!("    {}", fmt);
  state().scheduler.idle();
}

fn enable_nxe_bit() {
  use x86::msr::{IA32_EFER, rdmsr, wrmsr};

  let nxe_bit = 1 << 11;
  unsafe {
    let efer = rdmsr(IA32_EFER);
    wrmsr(IA32_EFER, efer | nxe_bit);
  }
}

fn enable_write_protect_bit() {
  use x86::controlregs::{cr0, cr0_write};

  let wp_bit = 1 << 16;
  unsafe { cr0_write(cr0() | wp_bit) };
}

pub fn test_call() {
  println!("I am a function");
  println!("I got called!");
}

#[no_mangle]
pub extern "C" fn rust_main(multiboot_information_address: usize) {

  let boot_info = unsafe {
    multiboot2::load(multiboot_information_address)
  };

  enable_nxe_bit();
  enable_write_protect_bit();

  // set up guard page and map the heap pages
  memory::init(boot_info);

  io::initialize();

  println!("Time is {}", io::timer::real_time().secs);

  println!("{:?}", memory::frame_allocator());

  use alloc::boxed::Box;
  use collections::Vec;
  let mut vec:Box<Vec<&'static str>> = Box::new(Vec::new());

  println!("{} {}", unsafe { tiered_allocator::BOOTSTRAP_ALLOCS }, unsafe { tiered_allocator::BOOTSTRAP_ALLOC_SIZE } );

  let my_str = "happy days are here";
  // Fails with PF if push count > 8192 because we don't support slabs that large.
  for _ in 0..8192 {
    vec.push(my_str);
  }

  println!("Created a vector with {} items?  Bananas. {}", vec.len(), vec[127]);
  
  debug!();

  state().scheduler.schedule();

  state().scheduler.idle();
}

/// Provide an easy, globally accessible function to get access to State
pub fn state() -> &'static mut state::State {
  state::state()
}

#[no_mangle]
pub extern "C" fn rust_interrupt_handler(ctx: &io::interrupts::InterruptContext) {
  io::interrupts::rust_interrupt_handler(ctx);
}

#[lang = "eh_personality"] extern fn eh_personality() {}
