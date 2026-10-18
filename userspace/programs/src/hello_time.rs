//! Hello Time userspace test (std version)
//!
//! Tests basic Ring 3 execution with time syscall and output.

use libbreenix::time::{clock_gettime, CLOCK_MONOTONIC};
use libbreenix::Timespec;
use std::process;

fn main() {
    // Get current time using clock_gettime (CLOCK_MONOTONIC)
    let mut ts = Timespec::new();

    if clock_gettime(CLOCK_MONOTONIC, &mut ts).is_err() {
        print!("Hello from userspace! clock_gettime failed\n");
        process::exit(1);
    }

    // Print greeting with time info
    let ticks_ns = ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64;
    print!("Hello from userspace! Current time: {} ticks\n", ticks_ns);

    // Exit cleanly with code 0
    process::exit(0);
}
