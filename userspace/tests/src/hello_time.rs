//! Hello Time userspace test (std version)
//!
//! Tests basic Ring 3 execution with time syscall and output.

use std::process;

extern "C" {
    fn clock_gettime(clk_id: i32, tp: *mut Timespec) -> i32;
}

const CLOCK_MONOTONIC: i32 = 1;

#[repr(C)]
struct Timespec {
    tv_sec: i64,
    tv_nsec: i64,
}

fn main() {
    // Get current time using clock_gettime (CLOCK_MONOTONIC)
    let mut ts = Timespec { tv_sec: 0, tv_nsec: 0 };
    let ret = unsafe { clock_gettime(CLOCK_MONOTONIC, &mut ts) };

    if ret != 0 {
        print!("Hello from userspace! clock_gettime failed\n");
        process::exit(1);
    }

    // Print greeting with time info
    let ticks_ns = ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64;
    print!("Hello from userspace! Current time: {} ticks\n", ticks_ns);

    // Exit cleanly with code 0
    process::exit(0);
}
