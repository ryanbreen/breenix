//! Compile-time ELF fixture for the coreproof binary scan, never a boot input.
#![no_std]
#![no_main]

use core::fmt::Write;

struct Stdout;

impl Write for Stdout {
    fn write_str(&mut self, text: &str) -> core::fmt::Result {
        libbreenix::io::stdout().write_str(text).map_err(|_| core::fmt::Error)?;
        Ok(())
    }
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    let mut ts = libbreenix::Timespec::new();
    if libbreenix::time::clock_gettime(libbreenix::time::CLOCK_MONOTONIC, &mut ts).is_err() {
        libbreenix::process::exit(1);
    }
    let ticks = ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64;
    if writeln!(Stdout, "Hello from userspace! Current time: {ticks} ticks").is_err() {
        libbreenix::process::exit(1);
    }
    libbreenix::process::exit(0);
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
    libbreenix::process::exit(1);
}
