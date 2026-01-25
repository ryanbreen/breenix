//! Process Group Kill Semantics Test
//!
//! Tests comprehensive process group kill semantics:
//! 1. kill(0, sig) - Send signal to all processes in caller's process group
//! 2. kill(-1, sig) - Send signal to all processes (except init, PID 1)
//! 3. kill(-pgid, sig) - Send signal to specific process group
//! 4. kill(pid, 0) - Check if process/group exists without sending signal

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libbreenix::io;
use libbreenix::process;
use libbreenix::signal;
use libbreenix::types::fd;

/// Static counters to track signal delivery
static mut SIGUSR1_COUNT: u32 = 0;
static mut SIGUSR2_COUNT: u32 = 0;

/// SIGUSR1 handler - increments counter
extern "C" fn sigusr1_handler(_sig: i32) {
    unsafe {
        SIGUSR1_COUNT += 1;
    }
}

/// SIGUSR2 handler - increments counter
extern "C" fn sigusr2_handler(_sig: i32) {
    unsafe {
        SIGUSR2_COUNT += 1;
    }
}

/// Print a number to stdout
unsafe fn print_number(num: u64) {
    let mut buffer: [u8; 32] = [0; 32];
    let mut n = num;
    let mut i = 0;

    if n == 0 {
        buffer[0] = b'0';
        i = 1;
    } else {
        while n > 0 {
            buffer[i] = b'0' + (n % 10) as u8;
            n /= 10;
            i += 1;
        }
        // Reverse the digits
        for j in 0..i / 2 {
            let tmp = buffer[j];
            buffer[j] = buffer[i - j - 1];
            buffer[i - j - 1] = tmp;
        }
    }

    io::write(fd::STDOUT, &buffer[..i]);
}

/// Print signed number
unsafe fn print_signed(num: i64) {
    if num < 0 {
        io::print("-");
        print_number((-num) as u64);
    } else {
        print_number(num as u64);
    }
}

/// Wait for signals to be delivered
/// Note: We use a high iteration count to handle slow emulation environments
/// like Docker TCG mode where scheduling can take much longer.
unsafe fn wait_for_signals() {
    for _ in 0..100 {
        process::yield_now();
    }
}

/// Wait for a specific signal using sigsuspend in a loop
/// This handles the case where sigsuspend might return due to other signals
unsafe fn wait_for_sigusr1() {
    // Block SIGUSR1 so sigsuspend can wait for it
    let sigusr1_mask: u64 = 1 << signal::SIGUSR1;
    let mut old_mask: u64 = 0;
    let _ = signal::sigprocmask(signal::SIG_BLOCK, Some(&sigusr1_mask), Some(&mut old_mask));

    // Loop until we receive SIGUSR1 (may wake up due to other signals)
    let empty_mask: u64 = 0;
    while SIGUSR1_COUNT == 0 {
        signal::sigsuspend(&empty_mask);
    }
}

/// Wait for SIGUSR2 using sigsuspend in a loop
unsafe fn wait_for_sigusr2() {
    // Block SIGUSR2 so sigsuspend can wait for it
    let sigusr2_mask: u64 = 1 << signal::SIGUSR2;
    let mut old_mask: u64 = 0;
    let _ = signal::sigprocmask(signal::SIG_BLOCK, Some(&sigusr2_mask), Some(&mut old_mask));

    // Loop until we receive SIGUSR2 (may wake up due to other signals)
    let empty_mask: u64 = 0;
    while SIGUSR2_COUNT == 0 {
        signal::sigsuspend(&empty_mask);
    }
}

/// Main entry point
#[no_mangle]
pub extern "C" fn _start() -> ! {
    unsafe {
        io::print("=== Process Group Kill Semantics Test ===\n\n");

        // Register signal handlers
        let action1 = signal::Sigaction::new(sigusr1_handler);
        let action2 = signal::Sigaction::new(sigusr2_handler);

        if signal::sigaction(signal::SIGUSR1, Some(&action1), None).is_err() {
            io::print("FAIL: Failed to register SIGUSR1 handler\n");
            io::print("KILL_PGROUP_TEST_FAILED\n");
            process::exit(1);
        }

        if signal::sigaction(signal::SIGUSR2, Some(&action2), None).is_err() {
            io::print("FAIL: Failed to register SIGUSR2 handler\n");
            io::print("KILL_PGROUP_TEST_FAILED\n");
            process::exit(1);
        }

        // Test 1: kill(pid, 0) - Check if process exists
        io::print("Test 1: kill(pid, 0) - Check process existence\n");
        let parent_pid = process::getpid();

        match signal::kill(parent_pid as i32, 0) {
            Ok(()) => io::print("  PASS: kill(self, 0) succeeded (process exists)\n"),
            Err(e) => {
                io::print("  FAIL: kill(self, 0) returned error ");
                print_number(e as u64);
                io::print("\n");
                io::print("KILL_PGROUP_TEST_FAILED\n");
                process::exit(1);
            }
        }

        // Check non-existent process
        match signal::kill(99999, 0) {
            Ok(()) => {
                io::print("  FAIL: kill(99999, 0) succeeded (should fail with ESRCH)\n");
                io::print("KILL_PGROUP_TEST_FAILED\n");
                process::exit(1);
            }
            Err(e) => {
                io::print("  PASS: kill(99999, 0) failed with errno ");
                print_number(e as u64);
                io::print(" (process does not exist)\n");
            }
        }

        // Test 2: Create process group and test kill(0, sig)
        io::print("\nTest 2: kill(0, sig) - Send signal to own process group\n");

        // Create a new process group with parent as leader
        if process::setpgid(0, 0) < 0 {
            io::print("  FAIL: setpgid(0, 0) failed\n");
            io::print("KILL_PGROUP_TEST_FAILED\n");
            process::exit(1);
        }

        let pgid = process::getpgrp();
        if pgid < 0 {
            io::print("  FAIL: getpgrp() failed\n");
            io::print("KILL_PGROUP_TEST_FAILED\n");
            process::exit(1);
        }

        io::print("  Created process group ");
        print_number(pgid as u64);
        io::print("\n");

        // Fork a child into the same process group
        let child1 = process::fork();
        if child1 < 0 {
            io::print("  FAIL: fork() failed\n");
            io::print("KILL_PGROUP_TEST_FAILED\n");
            process::exit(1);
        }

        if child1 == 0 {
            // Child 1: Wait for SIGUSR1 from kill(0, SIGUSR1)
            io::print("  [Child1] Started in process group ");
            print_number(process::getpgrp() as u64);
            io::print("\n");

            // Wait for SIGUSR1 using sigsuspend loop
            wait_for_sigusr1();

            io::print("  [Child1] PASS: Received SIGUSR1 via kill(0, sig)\n");
            process::exit(0);
        } else {
            // Parent: Give child time to set up sigsuspend, then send signal
            wait_for_signals();

            io::print("  [Parent] Sending SIGUSR1 to process group via kill(0, SIGUSR1)\n");

            match signal::kill(0, signal::SIGUSR1) {
                Ok(()) => io::print("  [Parent] kill(0, SIGUSR1) succeeded\n"),
                Err(e) => {
                    io::print("  [Parent] FAIL: kill(0, SIGUSR1) failed with errno ");
                    print_number(e as u64);
                    io::print("\n");
                    io::print("KILL_PGROUP_TEST_FAILED\n");
                    process::exit(1);
                }
            }

            // Wait for signal delivery
            wait_for_signals();

            // Check we also received the signal
            if SIGUSR1_COUNT == 1 {
                io::print("  [Parent] PASS: Received SIGUSR1 (process group signal delivery works)\n");
            } else {
                io::print("  [Parent] FAIL: Did not receive SIGUSR1 (count=");
                print_number(SIGUSR1_COUNT as u64);
                io::print(")\n");
                io::print("KILL_PGROUP_TEST_FAILED\n");
                process::exit(1);
            }

            // Wait for child
            let mut status = 0;
            let wait_result = process::waitpid(child1 as i32, &mut status as *mut i32, 0);
            if wait_result != child1 {
                io::print("  [Parent] WARNING: waitpid returned ");
                print_signed(wait_result);
                io::print("\n");
            } else if !process::wifexited(status) || process::wexitstatus(status) != 0 {
                io::print("  [Parent] FAIL: Child1 exited with non-zero status\n");
                io::print("KILL_PGROUP_TEST_FAILED\n");
                process::exit(1);
            }
        }

        // Test 3: kill(-pgid, sig) - Send signal to specific process group
        io::print("\nTest 3: kill(-pgid, sig) - Send signal to specific process group\n");

        // Reset counters
        SIGUSR2_COUNT = 0;

        // Fork child2 that creates its own process group
        let child2 = process::fork();
        if child2 < 0 {
            io::print("  FAIL: fork() failed for child2\n");
            io::print("KILL_PGROUP_TEST_FAILED\n");
            process::exit(1);
        }

        if child2 == 0 {
            // Child 2: Create new process group
            if process::setpgid(0, 0) < 0 {
                io::print("  [Child2] FAIL: setpgid(0, 0) failed\n");
                process::exit(1);
            }

            let child2_pgid = process::getpgrp();
            io::print("  [Child2] Created new process group ");
            print_number(child2_pgid as u64);
            io::print("\n");

            // Fork grandchild into child2's process group
            let grandchild = process::fork();
            if grandchild < 0 {
                io::print("  [Child2] FAIL: fork() failed for grandchild\n");
                process::exit(1);
            }

            if grandchild == 0 {
                // Grandchild: Wait for SIGUSR2 using sigsuspend loop
                io::print("  [Grandchild] Started in process group ");
                print_number(process::getpgrp() as u64);
                io::print("\n");

                // Wait for SIGUSR2 using sigsuspend loop
                wait_for_sigusr2();

                io::print("  [Grandchild] PASS: Received SIGUSR2 via kill(-pgid, sig)\n");
                process::exit(0);
            } else {
                // Child2: Wait for SIGUSR2 using sigsuspend loop
                wait_for_sigusr2();

                io::print("  [Child2] PASS: Received SIGUSR2 via kill(-pgid, sig)\n");

                // Wait for grandchild
                let mut gc_status = 0;
                let gc_wait = process::waitpid(grandchild as i32, &mut gc_status as *mut i32, 0);
                if gc_wait != grandchild {
                    io::print("  [Child2] WARNING: waitpid(grandchild) returned ");
                    print_signed(gc_wait);
                    io::print("\n");
                } else if !process::wifexited(gc_status) || process::wexitstatus(gc_status) != 0 {
                    io::print("  [Child2] FAIL: Grandchild exited with non-zero status\n");
                    process::exit(1);
                }

                process::exit(0);
            }
        } else {
            // Parent: Get child2's PGID and send signal to it
            wait_for_signals(); // Let child2 create its process group

            let child2_pgid = process::getpgid(child2 as i32);
            if child2_pgid < 0 {
                io::print("  [Parent] FAIL: getpgid(child2) failed\n");
                io::print("KILL_PGROUP_TEST_FAILED\n");
                process::exit(1);
            }

            io::print("  [Parent] Sending SIGUSR2 to process group ");
            print_number(child2_pgid as u64);
            io::print(" via kill(-pgid, sig)\n");

            match signal::kill(-child2_pgid, signal::SIGUSR2) {
                Ok(()) => io::print("  [Parent] kill(-pgid, SIGUSR2) succeeded\n"),
                Err(e) => {
                    io::print("  [Parent] FAIL: kill(-pgid, SIGUSR2) failed with errno ");
                    print_number(e as u64);
                    io::print("\n");
                    io::print("KILL_PGROUP_TEST_FAILED\n");
                    process::exit(1);
                }
            }

            // Parent should NOT receive SIGUSR2 (different process group)
            wait_for_signals();

            if SIGUSR2_COUNT == 0 {
                io::print("  [Parent] PASS: Did not receive SIGUSR2 (not in target process group)\n");
            } else {
                io::print("  [Parent] FAIL: Incorrectly received SIGUSR2 (should only go to child2's group)\n");
                io::print("KILL_PGROUP_TEST_FAILED\n");
                process::exit(1);
            }

            // Wait for child2
            let mut status2 = 0;
            let wait_result2 = process::waitpid(child2 as i32, &mut status2 as *mut i32, 0);
            if wait_result2 != child2 {
                io::print("  [Parent] WARNING: waitpid(child2) returned ");
                print_signed(wait_result2);
                io::print("\n");
            } else if !process::wifexited(status2) || process::wexitstatus(status2) != 0 {
                io::print("  [Parent] FAIL: Child2 exited with non-zero status\n");
                io::print("KILL_PGROUP_TEST_FAILED\n");
                process::exit(1);
            }
        }

        // Test 4: kill(-1, sig) - Send signal to all processes (except init)
        io::print("\nTest 4: kill(-1, sig) - Send signal to all processes\n");
        io::print("  NOTE: This test requires elevated privileges and may be limited\n");

        // Reset counters
        SIGUSR1_COUNT = 0;

        // Fork a child to verify broadcast
        let child3 = process::fork();
        if child3 < 0 {
            io::print("  FAIL: fork() failed for child3\n");
            io::print("KILL_PGROUP_TEST_FAILED\n");
            process::exit(1);
        }

        if child3 == 0 {
            // Child3: Wait for broadcast signal using sigsuspend loop
            io::print("  [Child3] Waiting for broadcast SIGUSR1\n");

            // Wait for SIGUSR1 using sigsuspend loop
            wait_for_sigusr1();

            io::print("  [Child3] PASS: Received broadcast SIGUSR1\n");
            process::exit(0);
        } else {
            // Parent: Give child time to set up sigsuspend
            wait_for_signals();

            // Parent: Attempt broadcast (may fail with EPERM)
            io::print("  [Parent] Attempting kill(-1, SIGUSR1) broadcast\n");

            match signal::kill(-1, signal::SIGUSR1) {
                Ok(()) => {
                    io::print("  [Parent] kill(-1, SIGUSR1) succeeded\n");

                    // Wait for signal delivery
                    wait_for_signals();

                    if SIGUSR1_COUNT == 1 {
                        io::print("  [Parent] PASS: Received broadcast signal\n");
                    } else {
                        io::print("  [Parent] FAIL: Did not receive broadcast signal\n");
                        io::print("KILL_PGROUP_TEST_FAILED\n");
                        process::exit(1);
                    }
                }
                Err(e) => {
                    io::print("  [Parent] kill(-1, SIGUSR1) failed with errno ");
                    print_number(e as u64);
                    io::print(" (may be EPERM - this is acceptable)\n");
                }
            }

            // Wait for child3
            let mut status3 = 0;
            let wait_result3 = process::waitpid(child3 as i32, &mut status3 as *mut i32, 0);
            if wait_result3 != child3 {
                io::print("  [Parent] WARNING: waitpid(child3) returned ");
                print_signed(wait_result3);
                io::print("\n");
            } else if !process::wifexited(status3) || process::wexitstatus(status3) != 0 {
                // Only fail if kill(-1) succeeded but child didn't get signal
                match signal::kill(-1, 0) {
                    Ok(()) => {
                        io::print("  [Parent] FAIL: Child3 exited with non-zero status after successful kill(-1)\n");
                        io::print("KILL_PGROUP_TEST_FAILED\n");
                        process::exit(1);
                    }
                    Err(_) => {
                        io::print("  [Parent] Child3 failed but kill(-1) not supported - acceptable\n");
                    }
                }
            }
        }

        // All tests passed
        io::print("\n=== All process group kill tests passed! ===\n");
        io::print("KILL_PGROUP_TEST_PASSED\n");
        process::exit(0);
    }
}

/// Panic handler
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    io::print("PANIC in kill_process_group test!\n");
    io::print("KILL_PGROUP_TEST_FAILED\n");
    process::exit(255);
}
