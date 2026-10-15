#![no_std]
#![no_main]

use libbreenix::{io, process};

// libbreenix's runtime supplies _start and exits with main's return value.
#[no_mangle]
pub extern "C" fn main(_argc: usize, _argv: *const *const u8) -> i32 {
    io::println("hello from {{name}}");
    0
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    io::println("{{name}} panicked");
    process::exit(101)
}
