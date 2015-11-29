#![feature(no_std, lang_items, const_fn, unique, core_str_ext)]
#![no_std]

extern crate rlibc;
extern crate spin;
extern crate multiboot2;

#[macro_use]
mod vga_buffer;

#[lang = "panic_fmt"]
extern fn panic_fmt(fmt: core::fmt::Arguments, file: &str, line: u32) -> ! {
  println!("\n\nPANIC in {} at line {}:", file, line);
  println!("    {}", fmt);
  loop{}
}

#[no_mangle]
pub extern fn rust_main(multiboot_information_address: usize) {
  vga_buffer::clear_screen();
  println!("Hello World{}", "!");
  println!("Whoop these numbers {} {}", 42, 100.0/33.0);

  let boot_info = unsafe{ multiboot2::load(multiboot_information_address) };
  let memory_map_tag = boot_info.memory_map_tag().expect("Memory map tag required");

  println!("memory areas:");
  for area in memory_map_tag.memory_areas() {
    println!("    start: 0x{:x}, length: 0x{:x}", area.base_addr, area.length);
  }

  let elf_sections_tag = boot_info.elf_sections_tag().expect("Elf-sections tag required");

  println!("kernel sections:");
  for section in elf_sections_tag.sections() {
    println!("    addr: 0x{:x}, size: 0x{:x}, flags: 0x{:x}", section.addr, section.size, section.flags);
  }


  panic!();

  loop{}
}

#[lang = "eh_personality"] extern fn eh_personality() {}

