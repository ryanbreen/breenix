//! mmap test suite (std version)
//!
//! Tests mmap, munmap, and mprotect syscalls.

use libbreenix::memory::{
    mmap, mprotect, munmap, MAP_ANONYMOUS, MAP_PRIVATE, PROT_READ, PROT_WRITE,
};
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
        assert_eq!(
            libbreenix::syscall::raw::syscall2(getrlimit_nr, 9, old_as.as_mut_ptr() as u64),
            0
        );
        assert_eq!(
            libbreenix::syscall::raw::syscall2(setrlimit_nr, 9, limited_as.as_ptr() as u64),
            0
        );
        assert_eq!(
            libbreenix::syscall::raw::syscall2(getrlimit_nr, 9, seen_as.as_mut_ptr() as u64),
            0
        );
    }
    assert_eq!(seen_as, limited_as);
    assert!(matches!(
        mmap(
            null_mut(),
            128usize << 20,
            PROT_READ | PROT_WRITE,
            MAP_PRIVATE | MAP_ANONYMOUS,
            -1,
            0
        ),
        Err(libbreenix::error::Error::Os(
            libbreenix::errno::Errno::ENOMEM
        ))
    ));
    unsafe {
        assert_eq!(
            libbreenix::syscall::raw::syscall2(setrlimit_nr, 9, old_as.as_ptr() as u64),
            0
        );
    }
    unsafe {
        let mut old_nofile = [0u64; 2];
        assert_eq!(
            libbreenix::syscall::raw::syscall2(getrlimit_nr, 7, old_nofile.as_mut_ptr() as u64),
            0
        );
        let fd = libbreenix::syscall::raw::syscall1(libbreenix::syscall::nr::DUP, 0);
        assert!((fd as i64) >= 3);
        let lowered = [fd, old_nofile[1]];
        assert_eq!(
            libbreenix::syscall::raw::syscall2(setrlimit_nr, 7, lowered.as_ptr() as u64),
            0
        );
        let descriptor = libbreenix::types::Fd::from_raw(fd);
        assert_eq!(
            libbreenix::io::dup2(descriptor, descriptor).unwrap(),
            descriptor
        );
        assert_eq!(
            libbreenix::syscall::raw::syscall2(setrlimit_nr, 7, old_nofile.as_ptr() as u64),
            0
        );
        assert_eq!(
            libbreenix::syscall::raw::syscall1(libbreenix::syscall::nr::CLOSE, fd),
            0
        );
    }
    println!("  Native resource limit ABI: PASS");

    // A large reservation must not require physical backing for untouched pages.
    // Read first and last pages before writing: fresh anonymous backing is zeroed.
    println!("Test 4: Sparse 128 MiB anonymous mapping...");
    let sparse_size = 128usize << 20;
    let rss_before = rss_kib();
    let sparse = mmap(
        null_mut(),
        sparse_size,
        PROT_READ | PROT_WRITE,
        MAP_PRIVATE | MAP_ANONYMOUS,
        -1,
        0,
    )
    .expect("large anonymous reservation");
    let rss_reserved = rss_kib();
    assert!(
        rss_reserved.saturating_sub(rss_before) < 1024,
        "reservation consumed physical frames: resident before={rss_before} KiB after={rss_reserved} KiB"
    );
    unsafe {
        assert_eq!(sparse.read_volatile(), 0);
        assert_eq!(sparse.add(sparse_size - 1).read_volatile(), 0);
        sparse.write_volatile(0x31);
        sparse.add(sparse_size - 1).write_volatile(0x72);
        assert_eq!(sparse.read_volatile(), 0x31);
        assert_eq!(sparse.add(sparse_size - 1).read_volatile(), 0x72);
    }
    // The two touched pages must show up, so the measure is not blind.
    let rss_touched = rss_kib();
    assert!(
        rss_touched >= rss_reserved + 8,
        "touched pages not resident: reserved={rss_reserved} KiB touched={rss_touched} KiB"
    );
    munmap(sparse, sparse_size).expect("unmap sparse reservation");
    println!("  Sparse mapping: PASS (resident before={rss_before} KiB reserved={rss_reserved} KiB touched={rss_touched} KiB)");
    partial_protection();
    untouched_signal_stack();
    shared_limits(setrlimit_nr, getrlimit_nr);
    descriptor_growth(setrlimit_nr, getrlimit_nr);

    println!("USERSPACE MMAP: ALL TESTS PASSED");
    std::process::exit(0);
}

/// This process's resident memory, so other programs' allocations stay out of the sample.
fn rss_kib() -> u64 {
    let pid = libbreenix::process::getpid().expect("getpid").raw();
    let text = std::fs::read_to_string(format!("/proc/{pid}/status")).expect("process status");
    text.lines()
        .find_map(|line| {
            line.strip_prefix("VmRSS:")
                .and_then(|value| value.split_whitespace().next())
                .and_then(|n| n.parse().ok())
        })
        .expect("VmRSS in process status")
}

fn partial_protection() {
    let mapping = mmap(
        null_mut(),
        3 * 4096,
        PROT_READ | PROT_WRITE,
        MAP_PRIVATE | MAP_ANONYMOUS,
        -1,
        0,
    )
    .unwrap();
    mprotect(unsafe { mapping.add(4096) }, 4096, 0).unwrap();
    unsafe {
        mapping.write_volatile(7);
        mapping.add(8192).write_volatile(9);
    }
    // Setting one reserved page RW must not permit the adjacent guard.
    mprotect(mapping, 3 * 4096, 0).unwrap();
    mprotect(unsafe { mapping.add(4096) }, 4096, PROT_READ | PROT_WRITE).unwrap();
    unsafe {
        mapping.add(4096).write_volatile(11);
    }
    match libbreenix::process::fork().unwrap() {
        libbreenix::process::ForkResult::Child => {
            libbreenix::signal::sigaction(
                libbreenix::signal::SIGSEGV,
                Some(&libbreenix::signal::Sigaction::default()),
                None,
            )
            .unwrap();
            unsafe {
                mapping.add(8192).write_volatile(13);
            }
            libbreenix::process::exit(1);
        }
        libbreenix::process::ForkResult::Parent(child) => {
            let mut status = 0;
            assert_eq!(
                libbreenix::process::waitpid(child.raw() as i32, &mut status, 0).unwrap(),
                child
            );
            assert!(libbreenix::process::wifsignaled(status));
            assert_eq!(
                libbreenix::process::wtermsig(status),
                libbreenix::signal::SIGSEGV
            );
        }
    }
    munmap(mapping, 3 * 4096).unwrap();
    println!("  Partial anonymous protection: PASS");
}

static SIGNAL_STACK: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
extern "C" fn on_signal(_: i32) {
    let byte = 1u8;
    unsafe {
        core::ptr::read_volatile(&byte);
    }
    SIGNAL_STACK.store(
        core::ptr::addr_of!(byte) as usize,
        std::sync::atomic::Ordering::SeqCst,
    );
}
fn untouched_signal_stack() {
    use libbreenix::signal::{self, Sigaction, StackT};
    let stack = mmap(
        null_mut(),
        16384,
        PROT_READ | PROT_WRITE,
        MAP_PRIVATE | MAP_ANONYMOUS,
        -1,
        0,
    )
    .unwrap();
    let alternate = StackT {
        ss_sp: stack as u64,
        ss_flags: 0,
        _pad: 0,
        ss_size: 8320,
    };
    signal::sigaltstack(Some(&alternate), None).unwrap();
    let mut action = Sigaction::new(on_signal);
    action.flags |= signal::SA_ONSTACK;
    signal::sigaction(signal::SIGUSR1, Some(&action), None).unwrap();
    signal::kill(libbreenix::process::getpid().unwrap().raw() as i32, signal::SIGUSR1).unwrap();
    let address = SIGNAL_STACK.load(std::sync::atomic::Ordering::SeqCst);
    assert!(address >= stack as usize && address < stack as usize + alternate.ss_size);
    signal::sigaltstack(Some(&StackT::default()), None).unwrap();
    munmap(stack, 16384).unwrap();
    println!("  Untouched alternate signal stack: PASS");
}

fn shared_limits(set: u64, get: u64) {
    let mut before = [0u64; 2];
    unsafe {
        assert_eq!(
            libbreenix::syscall::raw::syscall2(get, 1, before.as_mut_ptr() as u64),
            0
        );
    }
    let value = [123456u64, before[1]];
    let worker = std::thread::spawn(move || unsafe {
        assert_eq!(
            libbreenix::syscall::raw::syscall2(set, 1, value.as_ptr() as u64),
            0
        );
    });
    worker.join().unwrap();
    let mut after = [0u64; 2];
    unsafe {
        assert_eq!(
            libbreenix::syscall::raw::syscall2(get, 1, after.as_mut_ptr() as u64),
            0
        );
        assert_eq!(after, value);
        assert_eq!(
            libbreenix::syscall::raw::syscall2(set, 1, before.as_ptr() as u64),
            0
        );
    }
    println!("  Thread-shared resource limits: PASS");
}

fn descriptor_growth(set: u64, get: u64) {
    let mut before = [0u64; 2];
    unsafe {
        assert_eq!(
            libbreenix::syscall::raw::syscall2(get, 7, before.as_mut_ptr() as u64),
            0
        );
    }
    let value = [1024u64, before[1]];
    unsafe {
        assert_eq!(
            libbreenix::syscall::raw::syscall2(set, 7, value.as_ptr() as u64),
            0
        );
    }
    let high = libbreenix::types::Fd::from_raw(1000);
    assert_eq!(
        libbreenix::io::dup2(libbreenix::types::Fd::from_raw(0), high).unwrap(),
        high
    );
    libbreenix::io::close(high).unwrap();
    unsafe {
        assert_eq!(
            libbreenix::syscall::raw::syscall2(set, 7, before.as_ptr() as u64),
            0
        );
    }
    println!("  Growable descriptor limit: PASS");
}
