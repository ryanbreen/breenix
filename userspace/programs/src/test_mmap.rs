//! mmap test suite (std version)
//!
//! Tests mmap, munmap, and mprotect syscalls.

use libbreenix::memory::{mmap, munmap, mprotect, PROT_READ, PROT_WRITE, MAP_PRIVATE, MAP_ANONYMOUS};
use std::ptr::null_mut;

fn main() {
    println!("=== mmap Test Suite ===");

    // Test 1: Basic anonymous mmap
    println!("Test 1: Anonymous mmap...");
    let size = 4096usize; // One page
    let ptr = match mmap(
        null_mut(),
        size,
        PROT_READ | PROT_WRITE,
        MAP_PRIVATE | MAP_ANONYMOUS,
        -1,
        0,
    ) {
        Ok(p) => p,
        Err(_) => {
            println!("FAIL: mmap returned error");
            std::process::exit(1);
        }
    };
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
    if munmap(ptr, size).is_ok() {
        println!("  munmap succeeded: PASS");
    } else {
        println!("  munmap failed: FAIL");
        std::process::exit(1);
    }

    // Test 3: mprotect
    println!("Test 3: mprotect...");

    // Create a new mmap region for mprotect testing
    let ptr2 = match mmap(
        null_mut(),
        size,
        PROT_READ | PROT_WRITE,
        MAP_PRIVATE | MAP_ANONYMOUS,
        -1,
        0,
    ) {
        Ok(p) => p,
        Err(_) => {
            println!("  FAIL: mmap for mprotect test returned error");
            std::process::exit(1);
        }
    };
    println!("  mmap for mprotect test succeeded");

    // Write a pattern while we have write permission
    unsafe {
        for i in 0..size {
            *ptr2.add(i) = ((i * 2) & 0xFF) as u8;
        }
    }
    println!("  Write pattern succeeded");

    // Change protection to read-only
    if mprotect(ptr2, size, PROT_READ).is_ok() {
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
    if munmap(ptr2, size).is_ok() {
        println!("  Cleanup munmap: PASS");
    } else {
        println!("  Cleanup munmap: FAIL");
        std::process::exit(1);
    }

    println!("USERSPACE MMAP: ALL TESTS PASSED");
    std::process::exit(0);
}
