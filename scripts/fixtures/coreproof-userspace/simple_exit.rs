//! Compile-time ELF fixture for the coreproof binary scan, never a boot input.
#![no_std]
#![no_main]

#[no_mangle]
pub extern "C" fn _start() -> ! {
    libbreenix::process::exit(42);
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
    libbreenix::process::exit(1);
}
