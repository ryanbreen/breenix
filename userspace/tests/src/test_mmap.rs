//! mmap test suite (std version)
//!
//! Tests mmap, munmap, and mprotect syscalls.

use std::ptr::null_mut;

const PROT_READ: i32 = 0x1;
const PROT_WRITE: i32 = 0x2;
const MAP_PRIVATE: i32 = 0x02;
const MAP_ANONYMOUS: i32 = 0x20;
const MAP_FAILED: *mut u8 = !0usize as *mut u8;

extern "C" {
    fn mmap(addr: *mut u8, length: usize, prot: i32, flags: i32, fd: i32, offset: i64)
        -> *mut u8;
    fn munmap(addr: *mut u8, length: usize) -> i32;
    fn mprotect(addr: *mut u8, length: usize, prot: i32) -> i32;
}

fn main() {
    println!("=== mmap Test Suite ===");

    // Test 1: Basic anonymous mmap
    println!("Test 1: Anonymous mmap...");
    let size = 4096usize; // One page
    let ptr = unsafe {
        mmap(
            null_mut(),
            size,
            PROT_READ | PROT_WRITE,
            MAP_PRIVATE | MAP_ANONYMOUS,
            -1,
            0,
        )
    };

    if ptr == MAP_FAILED {
        println!("FAIL: mmap returned MAP_FAILED");
        std::process::exit(1);
    }
    println!("  mmap succeeded");

    // Write a pattern
    unsafe {
        for i in 0..size {
            *ptr.add(i) = (i & 0xFF) as u8;
        }
    }
    println!("  Write pattern succeeded");

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
        println!("  Read verification: PASS");
    } else {
        println!("  Read verification: FAIL");
        std::process::exit(1);
    }

    // Test 2: munmap
    println!("Test 2: munmap...");
    let result = unsafe { munmap(ptr, size) };
    if result == 0 {
        println!("  munmap succeeded: PASS");
    } else {
        println!("  munmap failed: FAIL");
        std::process::exit(1);
    }

    // Test 3: mprotect
    println!("Test 3: mprotect...");

    // Create a new mmap region for mprotect testing
    let ptr2 = unsafe {
        mmap(
            null_mut(),
            size,
            PROT_READ | PROT_WRITE,
            MAP_PRIVATE | MAP_ANONYMOUS,
            -1,
            0,
        )
    };

    if ptr2 == MAP_FAILED {
        println!("  FAIL: mmap for mprotect test returned MAP_FAILED");
        std::process::exit(1);
    }
    println!("  mmap for mprotect test succeeded");

    // Write a pattern while we have write permission
    unsafe {
        for i in 0..size {
            *ptr2.add(i) = ((i * 2) & 0xFF) as u8;
        }
    }
    println!("  Write pattern succeeded");

    // Change protection to read-only
    let prot_result = unsafe { mprotect(ptr2, size, PROT_READ) };
    if prot_result == 0 {
        println!("  mprotect to PROT_READ succeeded");
    } else {
        println!("  mprotect failed: FAIL");
        std::process::exit(1);
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
        println!("  Read after mprotect: PASS");
    } else {
        println!("  Read after mprotect: FAIL");
        std::process::exit(1);
    }

    // Clean up
    let result2 = unsafe { munmap(ptr2, size) };
    if result2 == 0 {
        println!("  Cleanup munmap: PASS");
    } else {
        println!("  Cleanup munmap: FAIL");
        std::process::exit(1);
    }

    println!("USERSPACE MMAP: ALL TESTS PASSED");
    std::process::exit(0);
}
