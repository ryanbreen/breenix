//! Spinner animation demo (std version)
//!
//! Displays a spinning animation character on the console.

use libbreenix::process::yield_now;
use libbreenix::time::{clock_gettime, CLOCK_MONOTONIC};
use libbreenix::Timespec;
use std::process;

/// Sleep for approximately the specified number of milliseconds.
/// Uses busy-wait polling with sched_yield() to be cooperative.
fn sleep_ms(ms: u64) {
    let mut start = Timespec::new();
    let _ = clock_gettime(CLOCK_MONOTONIC, &mut start);
    let target_ns = ms * 1_000_000;

    loop {
        let mut now = Timespec::new();
        let _ = clock_gettime(CLOCK_MONOTONIC, &mut now);

        let elapsed_sec = now.tv_sec - start.tv_sec;
        let elapsed_ns = if now.tv_nsec >= start.tv_nsec {
            elapsed_sec as u64 * 1_000_000_000 + (now.tv_nsec - start.tv_nsec) as u64
        } else {
            (elapsed_sec as u64 - 1) * 1_000_000_000
                + (1_000_000_000 + now.tv_nsec as u64 - start.tv_nsec as u64)
        };

        if elapsed_ns >= target_ns {
            break;
        }

        let _ = yield_now();
    }
}

/// Delay between spinner frames in milliseconds
const FRAME_DELAY_MS: u64 = 50;

/// Number of spinner frames to display
const TOTAL_FRAMES: usize = 100;

fn main() {
    println!("Spinner process starting!");

    // Spinner characters
    let spinner_chars = ['-', '\\', '|', '/'];

    // Spin for TOTAL_FRAMES iterations with FRAME_DELAY_MS between each
    for i in 0..TOTAL_FRAMES {
        // Use carriage return to go back to start of line, then print frame
        print!("\rSpinner: {}", spinner_chars[i % 4]);

        // Wait for the frame delay
        sleep_ms(FRAME_DELAY_MS);
    }

    // Final newline after spinner animation completes
    println!();

    println!("Spinner process finished!");

    // Exit cleanly with code 0
    process::exit(0);
}
