//! Spinner animation demo (std version)
//!
//! Displays a spinning animation character on the console.

use std::process;

#[repr(C)]
struct Timespec {
    tv_sec: i64,
    tv_nsec: i64,
}

extern "C" {
    fn clock_gettime(clk_id: i32, tp: *mut Timespec) -> i32;
    fn sched_yield() -> i32;
}

const CLOCK_MONOTONIC: i32 = 1;

/// Sleep for approximately the specified number of milliseconds.
/// Uses busy-wait polling with sched_yield() to be cooperative.
fn sleep_ms(ms: u64) {
    let mut start = Timespec { tv_sec: 0, tv_nsec: 0 };
    unsafe { clock_gettime(CLOCK_MONOTONIC, &mut start); }
    let target_ns = ms * 1_000_000;

    loop {
        let mut now = Timespec { tv_sec: 0, tv_nsec: 0 };
        unsafe { clock_gettime(CLOCK_MONOTONIC, &mut now); }

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

        unsafe { sched_yield(); }
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
