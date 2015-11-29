#![feature(no_std, lang_items, const_fn, unique, core_str_ext)]
#![no_std]

extern crate rlibc;

mod vga_buffer;

#[no_mangle]
pub unsafe extern fn rust_main() {
  vga_buffer::print_something();

  loop{}
}

#[lang = "eh_personality"] extern fn eh_personality() {}
#[lang = "panic_fmt"] extern fn panic_fmt() -> ! {loop{}}
