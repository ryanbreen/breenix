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

    // Exercise the native Linux ABI, rather than a libc's prlimit64 wrapper.
    #[cfg(target_arch = "x86_64")]
    let (getrlimit_nr, setrlimit_nr) = (97, 160);
    #[cfg(target_arch = "aarch64")]
    let (getrlimit_nr, setrlimit_nr) = (163, 164);
    let mut old_as = [0u64; 2];
    let mut seen_as = [0u64; 2];
    let limited_as = [64u64 << 20, u64::MAX];
    unsafe {
        assert_eq!(libbreenix::syscall::raw::syscall2(getrlimit_nr, 9, old_as.as_mut_ptr() as u64), 0);
        assert_eq!(libbreenix::syscall::raw::syscall2(setrlimit_nr, 9, limited_as.as_ptr() as u64), 0);
        assert_eq!(libbreenix::syscall::raw::syscall2(getrlimit_nr, 9, seen_as.as_mut_ptr() as u64), 0);
    }
    assert_eq!(seen_as, limited_as);
    assert!(matches!(mmap(null_mut(), 128usize << 20, PROT_READ | PROT_WRITE,
        MAP_PRIVATE | MAP_ANONYMOUS, -1, 0),
        Err(libbreenix::error::Error::Os(libbreenix::errno::Errno::ENOMEM))));
    unsafe {
        assert_eq!(libbreenix::syscall::raw::syscall2(setrlimit_nr, 9, old_as.as_ptr() as u64), 0);
    }
    #[cfg(target_arch = "x86_64")]
    unsafe {
        let mut old_nofile = [0u64; 2];
        assert_eq!(libbreenix::syscall::raw::syscall2(getrlimit_nr, 7, old_nofile.as_mut_ptr() as u64), 0);
        let fd = libbreenix::syscall::raw::syscall1(libbreenix::syscall::nr::DUP, 0);
        assert!((fd as i64) >= 3);
        let lowered = [fd, old_nofile[1]];
        assert_eq!(libbreenix::syscall::raw::syscall2(setrlimit_nr, 7, lowered.as_ptr() as u64), 0);
        assert_eq!(libbreenix::syscall::raw::syscall2(libbreenix::syscall::nr::DUP2, fd, fd), fd);
        assert_eq!(libbreenix::syscall::raw::syscall2(setrlimit_nr, 7, old_nofile.as_ptr() as u64), 0);
        assert_eq!(libbreenix::syscall::raw::syscall1(libbreenix::syscall::nr::CLOSE, fd), 0);
    }
    println!("  Native resource limit ABI: PASS");

    // A large reservation must not require physical backing for untouched pages.
    // Read first and last pages before writing: fresh anonymous backing is zeroed.
    println!("Test 4: Sparse 128 MiB anonymous mapping...");
    let sparse_size = 128usize << 20;
    let sparse = mmap(null_mut(), sparse_size, PROT_READ | PROT_WRITE,
        MAP_PRIVATE | MAP_ANONYMOUS, -1, 0).expect("large anonymous reservation");
    unsafe {
        assert_eq!(sparse.read_volatile(), 0);
        assert_eq!(sparse.add(sparse_size - 1).read_volatile(), 0);
        sparse.write_volatile(0x31);
        sparse.add(sparse_size - 1).write_volatile(0x72);
        assert_eq!(sparse.read_volatile(), 0x31);
        assert_eq!(sparse.add(sparse_size - 1).read_volatile(), 0x72);
    }
    munmap(sparse, sparse_size).expect("unmap sparse reservation");
    println!("  Sparse mapping: PASS");

    println!("USERSPACE MMAP: ALL TESTS PASSED");
    std::process::exit(0);
}
