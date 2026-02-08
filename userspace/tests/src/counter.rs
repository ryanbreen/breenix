//! Counter process (std version)
//!
//! Counts from 0 to 9, yielding between each count,
//! then exits with code 10.

extern "C" {
    fn sched_yield() -> i32;
}

fn main() {
    // Print greeting
    println!("Counter process starting!");

    // Count from 0 to 9, yielding between each count
    for i in 0..10u64 {
        println!("Counter: {}", i);

        // Yield to allow other processes to run
        unsafe {
            sched_yield();
        }

        // Do some busy work to simulate computation
        let mut sum = 0u64;
        for j in 0..100000u64 {
            sum = sum.wrapping_add(j);
        }

        // Prevent optimization
        if sum == 0 {
            println!("Unexpected!");
        }
    }

    println!("Counter process finished!");

    // Exit cleanly with code 10
    std::process::exit(10);
}
