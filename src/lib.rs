#![feature(no_std, lang_items, const_fn, unique, core_slice_ext, core_str_ext, iter_cmp, asm, step_by)]
#![no_std]

extern crate rlibc;
extern crate spin;
extern crate multiboot2;

extern crate cpuio;

#[macro_use]
extern crate bitflags;

#[macro_use(int)]
extern crate x86;

// Note: We must define vga_buffer first since it declared println!  Otherwise, no
// other mod can print.

#[macro_use]
mod vga_buffer;
mod memory;

mod io;

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
  vga_buffer::clear_screen();

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

  //enable_nxe_bit();
  //enable_write_protect_bit();
  //memory::remap_the_kernel(&mut frame_allocator, boot_info);
  
  unsafe {
    io::idt::setup();
  }
  io::keyboard::test();
}

/// Various data available on our stack when handling an interrupt.
///
/// Only `pub` because `rust_interrupt_handler` is.
#[repr(C, packed)]
pub struct InterruptContext {
  rsi: u64,
  rdi: u64,
  r11: u64,
  r10: u64,
  r9: u64,
  r8: u64,
  rdx: u64,
  rcx: u64,
  rax: u64,
  int_id: u32,
  _pad_1: u32,
  error_code: u32,
  _pad_2: u32,
}

/// Print our information about a CPU exception, and loop.
fn cpu_exception_handler(ctx: &InterruptContext) {

  // Print general information provided by x86::irq.
  println!("{}, error 0x{:x}",
           x86::irq::EXCEPTIONS[ctx.int_id as usize],
           ctx.error_code);

  // Provide detailed information about our error code if we know how to
  // parse it.
  match ctx.int_id {
      14 => {
          let err = x86::irq::PageFaultError::from_bits(ctx.error_code);
          println!("{:?}", err);
      }
      _ => {}
  }

  loop {}
}


/// Called from our assembly-language interrupt handlers to dispatch an
/// interrupt.
#[no_mangle]
pub unsafe extern "C" fn rust_interrupt_handler(ctx: &InterruptContext) {
  match ctx.int_id {
    0x00...0x0F => cpu_exception_handler(ctx),
    0x20 => { /* Timer. */ }
    0x21 => {
      println!("Keyboard bullshit");
      /*
      if let Some(input) = keyboard::read_char() {
        if input == '\r' {
          println!("");
        } else {
          print!("{}", input);
        }
      }
      */
    }
    0x80 => println!("Not actually Linux, sorry."),
    _ => {
      println!("UNKNOWN INTERRUPT #{}", ctx.int_id);
      loop {}
    }
  }

  io::idt::PICS.lock().notify_end_of_interrupt(ctx.int_id as u8);
}

#[lang = "eh_personality"] extern fn eh_personality() {}

