#![feature(no_std, lang_items, const_fn, unique, core_str_ext)]
#![no_std]

extern crate rlibc;
extern crate spin;

mod vga_buffer;

#[no_mangle]
pub unsafe extern fn rust_main() {
  use core::fmt::Write;
  vga_buffer::WRITER.lock().write_str("Hello again");
  write!(vga_buffer::WRITER.lock(), ", some numbers: {} {}", 42, 67);
  loop{}
}

#[lang = "eh_personality"] extern fn eh_personality() {}
#[lang = "panic_fmt"] extern fn panic_fmt() -> ! {loop{}}
