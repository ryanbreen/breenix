//! Timer test program (std version)
//!
//! Tests timing functionality using clock_gettime and sched_yield.

use libbreenix::process::yield_now;
use libbreenix::time::{clock_gettime, CLOCK_MONOTONIC};
use libbreenix::Timespec;

fn get_time_ms() -> u64 {
    let mut ts = Timespec::new();
    let _ = clock_gettime(CLOCK_MONOTONIC, &mut ts);
    (ts.tv_sec as u64) * 1000 + (ts.tv_nsec as u64) / 1_000_000
}

fn main() {
    println!("=== Timer Test Program ===");

    // Test 1: Get initial time
    let time1 = get_time_ms();
    println!("Test 1: Initial time = {} ms", time1);

    // Test 2: Yield 10 times and check time
    println!("Test 2: Yielding 10 times...");
    for _ in 0..10 {
        let _ = yield_now();
    }

    let time2 = get_time_ms();
    println!("        Time after yields = {} ms (delta = {} ms)", time2, time2.saturating_sub(time1));

    // Test 3: Busy wait and check time
    println!("Test 3: Busy waiting ~100ms...");
    for _ in 0..10_000_000u64 {
        // Busy wait
        unsafe { std::arch::asm!("nop"); }
    }

    let time3 = get_time_ms();
    println!("        Time after busy wait = {} ms (delta = {} ms)", time3, time3.saturating_sub(time2));

    // Test 4: Multiple rapid time calls
    println!("Test 4: Rapid time calls:");
    for i in 0..5 {
        let t = get_time_ms();
        println!("        Call {}: {} ms", i, t);
    }

    // Test 5: Long wait with progress
    println!("Test 5: Waiting 1 second with progress...");
    let start_time = get_time_ms();

    for i in 0..10 {
        // Wait ~100ms
        for _ in 0..100 {
            let _ = yield_now();
        }

        let current = get_time_ms();
        println!("        {}00ms: time = {} (elapsed = {} ms)", i + 1, current, current.saturating_sub(start_time));
    }

    // Final summary
    println!("\n=== Test Complete ===");
    let final_time = get_time_ms();
    println!("Total elapsed time: {} ms", final_time.saturating_sub(time1));

    if final_time == time1 {
        println!("ERROR: Timer is not incrementing!");
    } else {
        println!("SUCCESS: Timer is working!");
    }

    std::process::exit(0);
}
