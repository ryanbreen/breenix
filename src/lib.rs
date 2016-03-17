#![feature(alloc, allocator, box_syntax, core_intrinsics, macro_reexport, lang_items, const_fn, unique, asm, collections, stmt_expr_attributes)]
#![allocator]

#![no_std]

// Note: We must define macros first since it declared println!  Otherwise, no
// other mod can print.
#[macro_use]
mod macros;

extern crate collections;
extern crate alloc;
extern crate alloc_buddy_simple;
extern crate rlibc;

extern crate spin;

extern crate multiboot2;

extern crate cpuio;

#[macro_use]
extern crate bitflags;

#[macro_use(int)]
extern crate x86;

mod buffers;
mod constants;
mod event;
mod memory;
mod vga_writer;

mod io;

mod heap;
mod state;

use alloc::boxed::Box;
use core::fmt::Write;
use core::mem;
use collections::vec::Vec;
use collections::string::*;

#[no_mangle]
#[allow(non_snake_case)]
pub fn _Unwind_Resume() {
  println!("UNWIND!");
  loop {}
}

#[lang = "panic_fmt"]
extern fn panic_fmt(fmt: core::fmt::Arguments, file: &str, line: u32) -> ! {
  println!("\n\nPANIC in {} at line {}:", file, line);
  println!("    {}", fmt);
  loop{}
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

#[no_mangle]
pub extern "C" fn rust_main(multiboot_information_address: usize) {

  unsafe {
    // Do this first since we want all the goodness of collections from jump street.
    heap::initialize();
  }

  unsafe { buffers::ACTIVE_BUFFER.lock().clear(); }

  let boot_info = unsafe { multiboot2::load(multiboot_information_address) };
  let memory_map_tag = boot_info.memory_map_tag()
      .expect("Memory map tag required");
  let elf_sections_tag = boot_info.elf_sections_tag()
      .expect("Elf sections tag required");

  let kernel_start = elf_sections_tag.sections().map(|s| s.addr).min().unwrap();
  let kernel_end = elf_sections_tag.sections().map(|s| s.addr + s.size).max()
      .unwrap();

  let multiboot_start = multiboot_information_address;
  let multiboot_end = multiboot_start + (boot_info.total_size as usize);

  println!("kernel start: 0x{:x}, kernel end: 0x{:x}",
      kernel_start, kernel_end);
  println!("multiboot start: 0x{:x}, multiboot end: 0x{:x}",
      multiboot_start, multiboot_end);

  let mut frame_allocator = memory::AreaFrameAllocator::new(
      kernel_start as usize, kernel_end as usize, multiboot_start,
      multiboot_end, memory_map_tag.memory_areas());

  enable_nxe_bit();
  enable_write_protect_bit();
  memory::remap_the_kernel(&mut frame_allocator, boot_info);
  
  unsafe {
    io::interrupts::setup();

    use io::keyboard::{KeyEventScreenWriter,ToggleWatcher};
    state::register_key_event_listener(Box::new(KeyEventScreenWriter{}));
    state::register_key_event_listener(Box::new(ToggleWatcher{}));
  }

  let mut vec = Vec::<String>::new();

  for x in 0..10 {
    vec.push(format!("Entry {}", x));
  }

  println!("Hey, I made a vector in kernel space! {:?}", vec);

  debug!();

  loop {}
}

#[no_mangle]
pub extern "C" fn rust_interrupt_handler(ctx: &io::interrupts::InterruptContext) {
  io::interrupts::rust_interrupt_handler(ctx);
}

#[lang = "eh_personality"] extern fn eh_personality() {}

