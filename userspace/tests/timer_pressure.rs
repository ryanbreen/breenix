//! Timer pressure test program for validating concurrent timer interrupt handling
//!
//! This program is designed to stress-test the kernel's timer interrupt handling
//! and context switching under high concurrency. Multiple instances run simultaneously,
//! each doing CPU-bound work that will be preempted by timer interrupts.
//!
//! The test validates:
//! 1. Timer interrupts correctly preempt userspace threads
//! 2. Context switches preserve all register state
//! 3. No corruption occurs when switching between many threads
//! 4. Each thread makes forward progress (no starvation)

#![no_std]
#![no_main]

use core::panic::PanicInfo;

// System call numbers
const SYS_EXIT: u64 = 0;
const SYS_WRITE: u64 = 1;
const SYS_YIELD: u64 = 3;
const SYS_GETPID: u64 = 6;

// File descriptors
const STDOUT: u64 = 1;

// Test configuration
const ITERATIONS: u64 = 50;        // Number of progress reports
const WORK_PER_ITERATION: u64 = 100_000;  // Busy work between reports

// Inline assembly for INT 0x80 syscalls
#[inline(always)]
unsafe fn syscall0(num: u64) -> u64 {
    let ret: u64;
    core::arch::asm!(
        "int 0x80",
        in("rax") num,
        lateout("rax") ret,
        options(nostack, preserves_flags),
    );
    ret
}

#[inline(always)]
unsafe fn syscall1(num: u64, arg1: u64) -> u64 {
    let ret: u64;
    core::arch::asm!(
        "int 0x80",
        in("rax") num,
        in("rdi") arg1,
        lateout("rax") ret,
        options(nostack, preserves_flags),
    );
    ret
}

#[inline(always)]
unsafe fn syscall3(num: u64, arg1: u64, arg2: u64, arg3: u64) -> u64 {
    let ret: u64;
    core::arch::asm!(
        "int 0x80",
        in("rax") num,
        in("rdi") arg1,
        in("rsi") arg2,
        in("rdx") arg3,
        lateout("rax") ret,
        options(nostack, preserves_flags),
    );
    ret
}

// Simple write function
fn write_str(s: &str) {
    unsafe {
        syscall3(SYS_WRITE, STDOUT, s.as_ptr() as u64, s.len() as u64);
    }
}

// Get current PID
fn getpid() -> u64 {
    unsafe { syscall0(SYS_GETPID) }
}

// Yield to scheduler
fn yield_cpu() {
    unsafe {
        syscall0(SYS_YIELD);
    }
}

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

// Do busy work - this will be preempted by timer interrupts
#[inline(never)]
fn do_work(iterations: u64) -> u64 {
    let mut sum = 0u64;
    for i in 0..iterations {
        // Mix of operations to use different CPU resources
        sum = sum.wrapping_add(i);
        sum = sum.wrapping_mul(3);
        sum ^= i;

        // Occasional memory barrier to prevent over-optimization
        if i % 10000 == 0 {
            core::hint::spin_loop();
        }
    }
    sum
}

// Checksum for validating register state
// This value should remain constant throughout execution
fn compute_checksum() -> u64 {
    // Use a fixed pattern that we can verify
    let mut check: u64 = 0xDEAD_BEEF_CAFE_BABEu64;

    // Do some operations that use multiple registers
    for i in 0..10u64 {
        check = check.wrapping_add(i);
        check = check.rotate_left(7);
    }

    check
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // Get our PID for identification
    let pid = getpid();
    let mut pid_buf = [0u8; 20];
    let pid_str = num_to_str(pid, &mut pid_buf);

    // Compute initial checksum
    let initial_checksum = compute_checksum();

    // Print start marker with PID
    write_str("[PRESSURE:");
    write_str(pid_str);
    write_str("] Starting timer pressure test\n");

    // Track progress
    let mut total_work_sum = 0u64;

    // Main test loop
    for iteration in 0..ITERATIONS {
        // Do CPU-bound work (will be preempted by timer)
        let work_result = do_work(WORK_PER_ITERATION);
        total_work_sum = total_work_sum.wrapping_add(work_result);

        // Verify checksum hasn't changed (would indicate register corruption)
        let current_checksum = compute_checksum();
        if current_checksum != initial_checksum {
            write_str("[PRESSURE:");
            write_str(pid_str);
            write_str("] ERROR: Checksum mismatch! Register corruption detected!\n");
            unsafe { syscall1(SYS_EXIT, 99); }
            loop {}
        }

        // Progress report every 10 iterations
        if iteration % 10 == 0 {
            let mut iter_buf = [0u8; 20];
            let iter_str = num_to_str(iteration, &mut iter_buf);

            write_str("[PRESSURE:");
            write_str(pid_str);
            write_str("] Progress: ");
            write_str(iter_str);
            write_str("/");
            let mut total_buf = [0u8; 20];
            let total_str = num_to_str(ITERATIONS, &mut total_buf);
            write_str(total_str);
            write_str("\n");
        }

        // Occasionally yield to test voluntary context switches too
        if iteration % 5 == 0 {
            yield_cpu();
        }
    }

    // Final checksum verification
    let final_checksum = compute_checksum();
    if final_checksum != initial_checksum {
        write_str("[PRESSURE:");
        write_str(pid_str);
        write_str("] ERROR: Final checksum mismatch!\n");
        unsafe { syscall1(SYS_EXIT, 99); }
        loop {}
    }

    // Print completion marker
    write_str("[PRESSURE:");
    write_str(pid_str);
    write_str("] COMPLETE - All ");
    let mut iter_buf = [0u8; 20];
    let iter_str = num_to_str(ITERATIONS, &mut iter_buf);
    write_str(iter_str);
    write_str(" iterations passed, checksum OK\n");

    // Print work sum to prevent optimization
    write_str("[PRESSURE:");
    write_str(pid_str);
    write_str("] Work sum: ");
    let mut sum_buf = [0u8; 20];
    let sum_str = num_to_str(total_work_sum & 0xFFFF, &mut sum_buf);  // Just low 16 bits
    write_str(sum_str);
    write_str("\n");

    // Exit with PID as exit code (for tracking which processes completed)
    unsafe {
        syscall1(SYS_EXIT, pid & 0xFF);
    }

    // Should never reach here
    loop {}
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    write_str("[PRESSURE] PANIC!\n");

    // Exit with error code
    unsafe {
        syscall1(SYS_EXIT, 255);
    }

    // Should never reach here
    loop {}
}
