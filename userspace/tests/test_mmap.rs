#![no_std]
#![no_main]

use core::panic::PanicInfo;
use core::ptr::null_mut;

// Import libbreenix functions
use libbreenix::io::println;
use libbreenix::memory::{mmap, munmap, mprotect, PROT_READ, PROT_WRITE, MAP_PRIVATE, MAP_ANONYMOUS, MAP_FAILED};
use libbreenix::process::exit;

#[no_mangle]
pub extern "C" fn _start() -> ! {
    println("=== mmap Test Suite ===");

    // Test 1: Basic anonymous mmap
    println("Test 1: Anonymous mmap...");
    let size = 4096usize; // One page
    let ptr = mmap(
        null_mut(),
        size,
        PROT_READ | PROT_WRITE,
        MAP_PRIVATE | MAP_ANONYMOUS,
        -1,
        0,
    );

    if ptr == MAP_FAILED {
        println("FAIL: mmap returned MAP_FAILED");
        exit(1);
    }
    println("  mmap succeeded");

    // Write a pattern
    unsafe {
        for i in 0..size {
            *ptr.add(i) = (i & 0xFF) as u8;
        }
    }
    println("  Write pattern succeeded");

    // Read back and verify
    let mut verified = true;
    unsafe {
        for i in 0..size {
            if *ptr.add(i) != (i & 0xFF) as u8 {
                verified = false;
                break;
            }
        }
    }

    if verified {
        println("  Read verification: PASS");
    } else {
        println("  Read verification: FAIL");
        exit(1);
    }

    // Test 2: munmap
    println("Test 2: munmap...");
    let result = munmap(ptr, size);
    if result == 0 {
        println("  munmap succeeded: PASS");
    } else {
        println("  munmap failed: FAIL");
        exit(1);
    }

    // Test 3: mprotect
    println("Test 3: mprotect...");

    // Create a new mmap region for mprotect testing
    let ptr2 = mmap(
        null_mut(),
        size,
        PROT_READ | PROT_WRITE,
        MAP_PRIVATE | MAP_ANONYMOUS,
        -1,
        0,
    );

    if ptr2 == MAP_FAILED {
        println("  FAIL: mmap for mprotect test returned MAP_FAILED");
        exit(1);
    }
    println("  mmap for mprotect test succeeded");

    // Write a pattern while we have write permission
    unsafe {
        for i in 0..size {
            *ptr2.add(i) = ((i * 2) & 0xFF) as u8;
        }
    }
    println("  Write pattern succeeded");

    // Change protection to read-only
    let prot_result = mprotect(ptr2, size, PROT_READ);
    if prot_result == 0 {
        println("  mprotect to PROT_READ succeeded");
    } else {
        println("  mprotect failed: FAIL");
        exit(1);
    }

    // Verify we can still read the data
    let mut read_verified = true;
    unsafe {
        for i in 0..size {
            if *ptr2.add(i) != ((i * 2) & 0xFF) as u8 {
                read_verified = false;
                break;
            }
        }
    }

    if read_verified {
        println("  Read after mprotect: PASS");
    } else {
        println("  Read after mprotect: FAIL");
        exit(1);
    }

    // Clean up
    let result2 = munmap(ptr2, size);
    if result2 == 0 {
        println("  Cleanup munmap: PASS");
    } else {
        println("  Cleanup munmap: FAIL");
        exit(1);
    }

    println("USERSPACE MMAP: ALL TESTS PASSED");
    exit(0);
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    println("Test panic!");

    // Exit with error code 2
    exit(2);
}
