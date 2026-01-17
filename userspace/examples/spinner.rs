#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libbreenix::io::{print, println};
use libbreenix::process::{exit, yield_now};
use libbreenix::time::now_monotonic;

/// Sleep for approximately the specified number of milliseconds.
/// Uses busy-wait polling with yield_now() to be cooperative.
fn sleep_ms(ms: u64) {
    let start = now_monotonic();
    let target_ns = ms * 1_000_000; // Convert ms to ns

    loop {
        let now = now_monotonic();

        // Calculate elapsed nanoseconds
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

        // Yield to let other processes run while we wait
        yield_now();
    }
}

/// Delay between spinner frames in milliseconds
const FRAME_DELAY_MS: u64 = 50;

/// Number of spinner frames to display
const TOTAL_FRAMES: usize = 100;

#[no_mangle]
pub extern "C" fn _start() -> ! {
    println("Spinner process starting!");

    // Spinner characters
    let spinner_chars = [b'-', b'\\', b'|', b'/'];

    // Spin for TOTAL_FRAMES iterations with FRAME_DELAY_MS between each
    for i in 0..TOTAL_FRAMES {
        // Use carriage return to go back to start of line, then print frame
        // This creates the spinning animation effect on a single line
        print("\rSpinner: ");

        // Print spinner character
        let ch = spinner_chars[i % 4];
        let ch_buf = [ch];
        let ch_str = unsafe { core::str::from_utf8_unchecked(&ch_buf) };
        print(ch_str);

        // Wait for the frame delay
        sleep_ms(FRAME_DELAY_MS);
    }

    // Final newline after spinner animation completes
    println("");

    println("Spinner process finished!");

    // Exit cleanly with code 0
    exit(0);
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    println("Spinner process panic!");
    exit(4);
}