//! Hello Time userspace test
//!
//! Tests basic Ring 3 execution with time syscall and output.

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libbreenix::io;
use libbreenix::process;
use libbreenix::syscall::{nr, raw};

// Convert number to string (simple implementation)
fn num_to_str(mut num: u64, buf: &mut [u8]) -> &str {
    if num == 0 {
        buf[0] = b'0';
        return core::str::from_utf8(&buf[..1]).unwrap();
    }

    let mut i = 0;
    let mut digits = [0u8; 20]; // enough for u64

    while num > 0 {
        digits[i] = b'0' + (num % 10) as u8;
        num /= 10;
        i += 1;
    }

    // Reverse the digits
    for j in 0..i {
        buf[j] = digits[i - 1 - j];
    }

    core::str::from_utf8(&buf[..i]).unwrap()
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // DEBUGGING: First try a breakpoint to test kernel transitions
    #[cfg(target_arch = "x86_64")]
    unsafe {
        core::arch::asm!("int 3", options(nomem, nostack));
    }
    #[cfg(target_arch = "aarch64")]
    unsafe {
        core::arch::asm!("brk #3", options(nomem, nostack));
    }

    // Get current time using the legacy GET_TIME syscall
    let ticks = unsafe { raw::syscall0(nr::GET_TIME) };

    // Print greeting
    io::print("Hello from userspace! Current time: ");

    // Convert ticks to string and print
    let mut buf = [0u8; 20];
    let time_str = num_to_str(ticks, &mut buf);
    io::print(time_str);

    io::println(" ticks");

    // Exit cleanly with code 0
    process::exit(0);
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    io::print("Userspace panic!\n");
    process::exit(1);
}
