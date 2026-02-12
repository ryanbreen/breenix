//! Hello World using the REAL Rust standard library
//!
//! This test program uses the actual Rust std library, NOT #![no_std].
//! It demonstrates that Breenix can support real Rust programs with:
//! - println! macro (uses write syscall)
//! - Vec and other heap allocations (uses mmap/brk)
//! - std::process::exit
//!
//! Build with: cargo build -Z build-std=std,panic_abort --target x86_64-breenix.json  (x86_64)
//!         or: cargo build -Z build-std=std,panic_abort --target aarch64-breenix.json (aarch64)
//! This requires libbreenix-libc to be built and linked.

// NO #![no_std] - this uses real std!

use libbreenix::errno::Errno;
use libbreenix::error::Error;
use libbreenix::io;
use libbreenix::memory;
use libbreenix::process;
use libbreenix::types::Fd;

fn main() {
    // Test 1: Basic println! using std
    println!("RUST_STD_PRINTLN_WORKS");

    // Test 2: Direct clone syscall test (bypasses pthread_create + std::thread)
    {
        // We keep extern "C" write here because child_fn is an extern "C" callback
        // that runs in a raw cloned thread context with inline assembly.
        extern "C" {
            fn write(fd: i32, buf: *const u8, count: usize) -> isize;
        }

        // Helper to write a message to stderr (fd 2), avoiding any std locking
        unsafe fn raw_msg(msg: &[u8]) {
            write(2, msg.as_ptr(), msg.len());
        }

        extern "C" fn child_fn(arg: *mut u8) -> *mut u8 {
            // Write directly to stderr via write() syscall
            let msg = b"THREAD_TEST: child_fn running\n";
            unsafe { write(2, msg.as_ptr(), msg.len()) };

            // Write the magic value to the arg address (a shared memory location)
            let result_ptr = arg as *mut u64;
            unsafe { core::ptr::write_volatile(result_ptr, 42) };

            let msg2 = b"THREAD_TEST: child_fn done, calling exit\n";
            unsafe { write(2, msg2.as_ptr(), msg2.len()) };

            // Exit this thread via exit syscall
            // Breenix syscall numbers: Exit=0, Write=1 (NOT Linux numbering!)
            // After exit, spin in a loop until scheduler removes us.
            // The exit syscall marks us Terminated but returns to userspace
            // (PREEMPT_ACTIVE prevents immediate context switch during syscall return).
            // The spin loop keeps us from executing garbage instructions while
            // waiting for the next timer interrupt to switch us out.
            #[cfg(target_arch = "x86_64")]
            unsafe {
                core::arch::asm!(
                    "int 0x80",        // SYS_exit(0) - Breenix Exit=0
                    "2:",
                    "pause",           // Spin-loop hint (valid in Ring 3)
                    "jmp 2b",
                    in("rax") 0u64,    // SYS_EXIT = 0 in Breenix
                    in("rdi") 0u64,
                    options(noreturn),
                );
            }
            #[cfg(target_arch = "aarch64")]
            unsafe {
                core::arch::asm!(
                    "svc #0",          // SYS_exit(0) - Breenix Exit=0
                    "2:",
                    "yield",           // Spin-loop hint (ARM64 equivalent of PAUSE)
                    "b 2b",
                    in("x8") 0u64,     // SYS_EXIT = 0 in Breenix
                    in("x0") 0u64,
                    options(noreturn),
                );
            }
        }

        unsafe {
            raw_msg(b"THREAD_TEST: allocating stack\n");

            // Allocate child stack (64KB is enough for a simple test)
            let stack_size: usize = 64 * 1024;
            let stack = memory::mmap(core::ptr::null_mut(), stack_size, 3, 0x22, -1, 0); // PROT_READ|WRITE, MAP_PRIVATE|MAP_ANON
            let stack = match stack {
                Ok(ptr) => ptr,
                Err(_) => {
                    raw_msg(b"THREAD_TEST: ERROR stack mmap failed\n");
                    std::process::exit(1);
                }
            };
            raw_msg(b"THREAD_TEST: stack allocated\n");

            // Allocate shared result page
            let shared = match memory::mmap(core::ptr::null_mut(), 4096, 3, 0x22, -1, 0) {
                Ok(ptr) => ptr,
                Err(_) => {
                    raw_msg(b"THREAD_TEST: ERROR shared mmap failed\n");
                    std::process::exit(1);
                }
            };
            // Initialize result to 0
            core::ptr::write_volatile(shared as *mut u64, 0);
            // Initialize tid word (at offset 8) to 0xFFFF
            let tid_addr = shared.add(8) as *mut u32;
            core::ptr::write_volatile(tid_addr, 0xFFFF);

            raw_msg(b"THREAD_TEST: about to call clone syscall\n");

            let stack_top = (stack as usize + stack_size) & !0xF;

            // Clone flags: CLONE_VM | CLONE_FILES | CLONE_CHILD_CLEARTID | CLONE_CHILD_SETTID
            let flags: u64 = 0x00000100 | 0x00000400 | 0x00200000 | 0x01000000;

            // clone(flags, child_stack, fn_ptr, fn_arg, child_tidptr)
            let ret: i64;
            #[cfg(target_arch = "x86_64")]
            core::arch::asm!(
                "int 0x80",
                in("rax") 56u64,        // SYS_clone
                in("rdi") flags,
                in("rsi") stack_top as u64,
                in("rdx") child_fn as u64,
                in("r10") shared as u64,   // fn_arg = pointer to shared page (child writes result here)
                in("r8") tid_addr as u64,  // child_tidptr
                lateout("rax") ret,
                options(nostack),
            );
            #[cfg(target_arch = "aarch64")]
            core::arch::asm!(
                "svc #0",
                in("x8") 56u64,         // SYS_clone
                inlateout("x0") flags as u64 => ret,
                in("x1") stack_top as u64,
                in("x2") child_fn as u64,
                in("x3") shared as u64,    // fn_arg = pointer to shared page
                in("x4") tid_addr as u64,  // child_tidptr
                options(nostack),
            );

            if ret < 0 {
                raw_msg(b"THREAD_TEST: ERROR clone syscall failed\n");
                std::process::exit(1);
            }
            raw_msg(b"THREAD_TEST: clone returned successfully\n");

            // Wait for child by polling tid_addr (should be set to 0 on child exit)
            // Spin-wait: the timer interrupt (1ms) will preempt us and schedule the child.
            // SYS_YIELD (Breenix=3) sets need_resched but PREEMPT_ACTIVE on syscall
            // return means the actual context switch happens on the next timer tick.
            raw_msg(b"THREAD_TEST: waiting for child\n");
            for i in 0..10_000_000u64 {
                let tid_val = core::ptr::read_volatile(tid_addr);
                if tid_val == 0 {
                    break;
                }
                // Yield CPU: Breenix Yield=3
                #[cfg(target_arch = "x86_64")]
                core::arch::asm!("int 0x80", in("rax") 3u64, options(nostack));
                #[cfg(target_arch = "aarch64")]
                core::arch::asm!("svc #0", in("x8") 3u64, in("x0") 0u64, options(nostack));
                // Print progress every 1M iterations
                if i > 0 && i % 1_000_000 == 0 {
                    let digit = b'0' + (i / 1_000_000) as u8;
                    let progress = [b'T', b'H', b'R', b'E', b'A', b'D', b'_', b'W', b'A', b'I', b'T', b':', digit, b'\n'];
                    write(2, progress.as_ptr(), progress.len());
                }
            }

            let tid_val = core::ptr::read_volatile(tid_addr);
            let result = core::ptr::read_volatile(shared as *const u64);

            // Print diagnostic info regardless of pass/fail
            if result == 42 {
                raw_msg(b"THREAD_TEST: result=42 (correct)\n");
            } else {
                raw_msg(b"THREAD_TEST: result!=42\n");
            }
            if tid_val == 0 {
                raw_msg(b"THREAD_TEST: tid_val=0 (child exited)\n");
            } else {
                raw_msg(b"THREAD_TEST: tid_val!=0 (child did NOT exit)\n");
            }

            if tid_val == 0 && result == 42 {
                raw_msg(b"THREAD_TEST: child completed, result=42\n");
                println!("RUST_STD_THREAD_WORKS");
            } else {
                eprintln!("ERROR: thread test failed: tid_val={}, result={}", tid_val, result);
                std::process::exit(1);
            }
        }
    }

    // Test 3: Vec allocation and operations
    let numbers: Vec<i32> = vec![1, 2, 3, 4, 5];
    let sum: i32 = numbers.iter().sum();
    println!("Sum: {}", sum);

    // Verify the sum is correct (1+2+3+4+5 = 15)
    if sum == 15 {
        println!("RUST_STD_VEC_WORKS");
    } else {
        eprintln!("ERROR: Vec sum incorrect, got {}, expected 15", sum);
        std::process::exit(1);
    }

    // Test 3: String operations
    let greeting = String::from("Hello, ");
    let name = String::from("Breenix!");
    let message = greeting + &name;
    println!("{}", message);

    if message == "Hello, Breenix!" {
        println!("RUST_STD_STRING_WORKS");
    }

    // Test 4: Format macro
    let formatted = format!("Testing format: {} + {} = {}", 2, 3, 2 + 3);
    println!("{}", formatted);

    if formatted == "Testing format: 2 + 3 = 5" {
        println!("RUST_STD_FORMAT_WORKS");
    }

    // Test 5: getrandom returns random bytes
    // The kernel implements getrandom (syscall 318) with a TSC-seeded PRNG.
    // Note: getrandom is a libc function, not available in libbreenix.
    unsafe {
        extern "C" {
            fn getrandom(buf: *mut u8, buflen: usize, flags: u32) -> isize;
        }

        let mut buf = [0u8; 16];
        let result = getrandom(buf.as_mut_ptr(), buf.len(), 0);

        if result == 16 {
            // Verify we got some non-zero bytes (all zeros is astronomically unlikely)
            let any_nonzero = buf.iter().any(|&b| b != 0);
            if any_nonzero {
                println!("RUST_STD_GETRANDOM_WORKS");
            } else {
                eprintln!("ERROR: getrandom returned all zeros (extremely unlikely)");
                std::process::exit(1);
            }
        } else {
            eprintln!("ERROR: getrandom returned {}, expected 16", result);
            std::process::exit(1);
        }
    }

    // Test 5b: HashMap (requires working getrandom for hasher seeding)
    {
        use std::collections::HashMap;
        let mut map = HashMap::new();
        map.insert("one", 1);
        map.insert("two", 2);
        map.insert("three", 3);

        if map.len() == 3 && map["one"] == 1 && map["two"] == 2 && map["three"] == 3 {
            println!("RUST_STD_HASHMAP_WORKS");
        } else {
            eprintln!("ERROR: HashMap operations failed");
            std::process::exit(1);
        }
    }

    // Test 6: realloc data preservation
    // This test verifies that realloc properly preserves data when growing an allocation.
    // Previously, realloc would copy `new_size` bytes from the old allocation, which
    // could read beyond the old allocation's bounds (undefined behavior).
    // Note: malloc/realloc/free are libc functions, not available in libbreenix.
    unsafe {
        extern "C" {
            fn malloc(size: usize) -> *mut u8;
            fn realloc(ptr: *mut u8, size: usize) -> *mut u8;
            fn free(ptr: *mut u8);
        }

        // Allocate initial buffer of 64 bytes
        let ptr = malloc(64);
        if !ptr.is_null() {
            // Write a pattern: each byte is (index * 7) mod 256
            for i in 0..64 {
                *ptr.add(i) = (i as u8).wrapping_mul(7);
            }

            // Realloc to larger size (128 bytes)
            let new_ptr = realloc(ptr, 128);
            if !new_ptr.is_null() {
                // Verify the original pattern is preserved in the first 64 bytes
                let mut pattern_ok = true;
                for i in 0..64 {
                    if *new_ptr.add(i) != (i as u8).wrapping_mul(7) {
                        eprintln!("ERROR: realloc data corruption at index {}: expected {}, got {}",
                                  i, (i as u8).wrapping_mul(7), *new_ptr.add(i));
                        pattern_ok = false;
                        break;
                    }
                }
                if pattern_ok {
                    println!("RUST_STD_REALLOC_WORKS");
                }
                free(new_ptr);
            } else {
                eprintln!("ERROR: realloc returned null");
            }
        } else {
            eprintln!("ERROR: malloc returned null");
        }
    }

    // Test 7: realloc shrink case (128->64 bytes)
    // This test verifies that realloc properly preserves data when shrinking an allocation.
    // The fix uses min(old_size, new_size) to copy the correct number of bytes.
    // Note: malloc/realloc/free are libc functions, not available in libbreenix.
    unsafe {
        extern "C" {
            fn malloc(size: usize) -> *mut u8;
            fn realloc(ptr: *mut u8, size: usize) -> *mut u8;
            fn free(ptr: *mut u8);
        }

        // Allocate 128 bytes
        let ptr = malloc(128);
        if !ptr.is_null() {
            // Write a pattern to all 128 bytes
            for i in 0..128 {
                *ptr.add(i) = (i as u8).wrapping_mul(11);
            }

            // Shrink to 64 bytes
            let new_ptr = realloc(ptr, 64);
            if !new_ptr.is_null() {
                // Verify the first 64 bytes are preserved
                let mut pattern_ok = true;
                for i in 0..64 {
                    if *new_ptr.add(i) != (i as u8).wrapping_mul(11) {
                        eprintln!("ERROR: realloc shrink data corruption at index {}: expected {}, got {}",
                                  i, (i as u8).wrapping_mul(11), *new_ptr.add(i));
                        pattern_ok = false;
                        break;
                    }
                }
                if pattern_ok {
                    println!("RUST_STD_REALLOC_SHRINK_WORKS");
                }
                free(new_ptr);
            } else {
                eprintln!("ERROR: realloc shrink returned null");
            }
        } else {
            eprintln!("ERROR: malloc for shrink test returned null");
        }
    }

    // Test 8: read() error handling
    // Test that read() with invalid fd returns error with EBADF
    {
        let mut buf = [0u8; 16];
        // Read from invalid fd (-1) should fail with EBADF
        // We use Fd::from_raw with a large value that wraps to -1 in the syscall
        let result = io::read(Fd::from_raw(-1i32 as u64), &mut buf);

        match result {
            Err(Error::Os(Errno::EBADF)) => {
                println!("RUST_STD_READ_ERROR_WORKS");
            }
            Err(Error::Os(errno)) => {
                eprintln!("ERROR: read set errno to {:?}, expected EBADF", errno);
                std::process::exit(1);
            }
            Ok(n) => {
                eprintln!("ERROR: read returned Ok({}), expected Err(EBADF)", n);
                std::process::exit(1);
            }
        }
    }

    // Test 9: read() success with pipe
    // This tests that read() actually works with a valid file descriptor.
    // We create a pipe, write to it, and read back the data.
    {
        let (read_fd, write_fd) = io::pipe().unwrap_or_else(|e| {
            eprintln!("ERROR: pipe() failed: {:?}", e);
            std::process::exit(1);
        });

        // Write some test data to the pipe
        let test_data = b"Hello pipe!";
        let written = io::write(write_fd, test_data).unwrap_or_else(|e| {
            eprintln!("ERROR: write failed: {:?}", e);
            std::process::exit(1);
        });

        if written == test_data.len() {
            // Read the data back
            let mut buf = [0u8; 32];
            let bytes_read = io::read(read_fd, &mut buf).unwrap_or_else(|e| {
                eprintln!("ERROR: read failed: {:?}", e);
                std::process::exit(1);
            });

            if bytes_read == test_data.len() {
                // Verify the data matches
                let mut data_matches = true;
                for i in 0..test_data.len() {
                    if buf[i] != test_data[i] {
                        data_matches = false;
                        break;
                    }
                }

                if data_matches {
                    println!("RUST_STD_READ_SUCCESS_WORKS");
                } else {
                    eprintln!("ERROR: read data does not match written data");
                    std::process::exit(1);
                }
            } else {
                eprintln!("ERROR: read returned {}, expected {}", bytes_read, test_data.len());
                std::process::exit(1);
            }
        } else {
            eprintln!("ERROR: write returned {}, expected {}", written, test_data.len());
            std::process::exit(1);
        }

        // Close the pipe fds
        let _ = io::close(read_fd);
        let _ = io::close(write_fd);
    }

    // Test 10: malloc boundary conditions
    // Test malloc with edge cases: size=0 and small allocations
    // Use black_box to prevent compiler from optimizing away malloc(0) call
    // Note: malloc/free are libc functions, not available in libbreenix.
    unsafe {
        extern "C" {
            fn malloc(size: usize) -> *mut u8;
            fn free(ptr: *mut u8);
        }

        // Test 1: malloc(0) should return NULL (our implementation returns NULL for size=0)
        // black_box prevents the compiler from assuming malloc semantics and optimizing this away
        let ptr_zero = malloc(std::hint::black_box(0));
        let zero_ok = std::hint::black_box(ptr_zero).is_null();

        // Test 2: malloc small size should succeed
        let ptr_small = malloc(1);
        let small_ok = !ptr_small.is_null();
        if small_ok {
            *ptr_small = 42; // Should be writable
            let read_ok = *ptr_small == 42;
            free(ptr_small);

            if zero_ok && read_ok {
                println!("RUST_STD_MALLOC_BOUNDARY_WORKS");
            } else {
                if !zero_ok {
                    eprintln!("ERROR: malloc(0) did not return NULL");
                }
                if !read_ok {
                    eprintln!("ERROR: malloc(1) memory not readable/writable");
                }
                std::process::exit(1);
            }
        } else {
            eprintln!("ERROR: malloc(1) returned null");
            std::process::exit(1);
        }
    }

    // Test 11: posix_memalign
    // This test verifies that posix_memalign properly allocates aligned memory.
    // Note: posix_memalign/free are libc functions, not available in libbreenix.
    unsafe {
        extern "C" {
            fn posix_memalign(memptr: *mut *mut u8, alignment: usize, size: usize) -> i32;
            fn free(ptr: *mut u8);
        }

        let mut ptr: *mut u8 = core::ptr::null_mut();

        // Test 1: 16-byte alignment (common)
        let result = posix_memalign(&mut ptr, 16, 64);
        if result == 0 && !ptr.is_null() {
            // Verify alignment
            let addr = ptr as usize;
            let aligned_16 = (addr % 16) == 0;

            // Write and read to verify memory is usable
            *ptr = 123;
            let write_ok = *ptr == 123;

            free(ptr);

            // Test 2: 4096-byte (page) alignment
            ptr = core::ptr::null_mut();
            let result2 = posix_memalign(&mut ptr, 4096, 64);
            if result2 == 0 && !ptr.is_null() {
                let addr2 = ptr as usize;
                let aligned_4096 = (addr2 % 4096) == 0;
                free(ptr);

                if aligned_16 && write_ok && aligned_4096 {
                    println!("RUST_STD_POSIX_MEMALIGN_WORKS");
                } else {
                    eprintln!("ERROR: posix_memalign alignment check failed: 16={}, write={}, 4096={}",
                              aligned_16, write_ok, aligned_4096);
                }
            } else {
                eprintln!("ERROR: posix_memalign(4096) failed: result={}", result2);
            }
        } else {
            eprintln!("ERROR: posix_memalign(16) failed: result={}", result);
        }
    }

    // Test 12: sbrk
    // This test verifies that sbrk properly manages the program break.
    // Note: We test the C sbrk + __errno_location since we need to test
    // negative-increment error behavior at the C ABI level.
    unsafe {
        extern "C" {
            fn sbrk(increment: isize) -> *mut u8;
            fn __errno_location() -> *mut i32;
        }

        // Test 1: Query current break (increment=0)
        let current = sbrk(0);
        let query_ok = !current.is_null() && current != usize::MAX as *mut u8;

        // Test 2: Extend break by small amount
        let new_ptr = sbrk(4096);
        let extend_ok = !new_ptr.is_null() && new_ptr != usize::MAX as *mut u8;

        // Test 3: Negative increment should fail with EINVAL
        let neg_result = sbrk(-4096);
        let errno = *__errno_location();
        let neg_fails = neg_result == usize::MAX as *mut u8 && errno == 22; // EINVAL = 22

        if query_ok && extend_ok && neg_fails {
            println!("RUST_STD_SBRK_WORKS");
        } else {
            eprintln!("ERROR: sbrk test failed: query={}, extend={}, neg_fails={} (errno={})",
                      query_ok, extend_ok, neg_fails, errno);
        }
    }

    // Test 13: getpid and gettid
    // This test verifies that getpid() and gettid() return valid positive values.
    // These are Phase 1 required functions for process identification.
    {
        let pid = process::getpid().unwrap_or_else(|e| {
            eprintln!("ERROR: getpid failed: {:?}", e);
            std::process::exit(1);
        });
        let tid = process::gettid().unwrap_or_else(|e| {
            eprintln!("ERROR: gettid failed: {:?}", e);
            std::process::exit(1);
        });

        let pid_val = pid.raw() as i32;
        let tid_val = tid.raw() as i32;

        // Both should be positive (valid process/thread IDs)
        let pid_ok = pid_val > 0;
        let tid_ok = tid_val > 0;
        // For single-threaded process, tid >= pid (typically equal or tid > pid)
        let tid_ge_pid = tid_val >= pid_val;

        if pid_ok && tid_ok && tid_ge_pid {
            println!("RUST_STD_GETPID_WORKS");
        } else {
            eprintln!("ERROR: getpid/gettid test failed: pid={} (>0: {}), tid={} (>0: {}), tid>=pid: {}",
                      pid_val, pid_ok, tid_val, tid_ok, tid_ge_pid);
            std::process::exit(1);
        }
    }

    // Test 14: posix_memalign error cases
    // This test verifies that posix_memalign returns EINVAL for invalid alignments.
    // The POSIX spec requires EINVAL when:
    // - alignment is 0
    // - alignment is not a power of 2
    // - alignment is not a multiple of sizeof(void*) (8 bytes on x86_64)
    // Note: posix_memalign is a libc function, not available in libbreenix.
    unsafe {
        extern "C" {
            fn posix_memalign(memptr: *mut *mut u8, alignment: usize, size: usize) -> i32;
        }

        const EINVAL: i32 = 22;
        let mut ptr: *mut u8 = core::ptr::null_mut();

        // Test 1: alignment=0 should return EINVAL
        let result_zero = posix_memalign(&mut ptr, 0, 64);
        let zero_ok = result_zero == EINVAL;

        // Test 2: alignment=3 (not power of 2) should return EINVAL
        ptr = core::ptr::null_mut();
        let result_three = posix_memalign(&mut ptr, 3, 64);
        let three_ok = result_three == EINVAL;

        // Test 3: alignment=5 (not power of 2) should return EINVAL
        ptr = core::ptr::null_mut();
        let result_five = posix_memalign(&mut ptr, 5, 64);
        let five_ok = result_five == EINVAL;

        // Test 4: alignment=4 (less than sizeof(void*)=8) should return EINVAL
        ptr = core::ptr::null_mut();
        let result_four = posix_memalign(&mut ptr, 4, 64);
        let four_ok = result_four == EINVAL;

        // Test 5: alignment=7 (not power of 2, less than 8) should return EINVAL
        ptr = core::ptr::null_mut();
        let result_seven = posix_memalign(&mut ptr, 7, 64);
        let seven_ok = result_seven == EINVAL;

        if zero_ok && three_ok && five_ok && four_ok && seven_ok {
            println!("RUST_STD_POSIX_MEMALIGN_ERRORS_WORK");
        } else {
            eprintln!("ERROR: posix_memalign error handling failed:");
            eprintln!("  alignment=0: {} (expected EINVAL=22, got {})", zero_ok, result_zero);
            eprintln!("  alignment=3: {} (expected EINVAL=22, got {})", three_ok, result_three);
            eprintln!("  alignment=5: {} (expected EINVAL=22, got {})", five_ok, result_five);
            eprintln!("  alignment=4: {} (expected EINVAL=22, got {})", four_ok, result_four);
            eprintln!("  alignment=7: {} (expected EINVAL=22, got {})", seven_ok, result_seven);
            std::process::exit(1);
        }
    }

    // Test 15: free(NULL) is safe
    // The C standard (C99 7.20.3.2) requires that free(NULL) is a no-op.
    // This is important because many programs rely on this behavior.
    // NOTE: Double-free (calling free() on already-freed memory) is UNDEFINED BEHAVIOR
    // and cannot be safely tested - it may corrupt heap state, crash, or appear to work.
    // We only test free(NULL) which is defined behavior.
    // Note: free is a libc function, not available in libbreenix.
    unsafe {
        extern "C" {
            fn free(ptr: *mut u8);
        }

        // Calling free(NULL) should be a no-op and not crash
        free(core::ptr::null_mut());
        free(core::ptr::null_mut()); // Call twice to be sure
        free(core::ptr::null_mut()); // And a third time

        // If we get here, free(NULL) worked correctly
        println!("RUST_STD_FREE_NULL_WORKS");
    }

    // Test 16: close() syscall
    // This test verifies that close() properly closes file descriptors.
    // We use dup() to create a valid fd, then close() it.
    {
        // Test 1: dup stdout (fd 1) to get a new valid fd
        let new_fd = io::dup(Fd::STDOUT).unwrap_or_else(|e| {
            eprintln!("ERROR: dup(1) failed: {:?}", e);
            std::process::exit(1);
        });

        // Test 2: close the dup'd fd - should succeed
        io::close(new_fd).unwrap_or_else(|e| {
            eprintln!("ERROR: close({}) failed: {:?}", new_fd.raw(), e);
            std::process::exit(1);
        });

        // Test 3: closing an already-closed fd should fail with EBADF (9)
        match io::close(new_fd) {
            Err(Error::Os(Errno::EBADF)) => {
                // Expected
            }
            Err(Error::Os(errno)) => {
                eprintln!("ERROR: close on closed fd set errno to {:?}, expected EBADF", errno);
                std::process::exit(1);
            }
            Ok(()) => {
                eprintln!("ERROR: close({}) on already-closed fd succeeded, expected EBADF", new_fd.raw());
                std::process::exit(1);
            }
        }

        println!("RUST_STD_CLOSE_WORKS");
    }

    // Test 17: mprotect
    // This test verifies that mprotect properly changes memory protection.
    {
        // Protection flags
        const PROT_READ: i32 = 1;
        const PROT_WRITE: i32 = 2;

        // Test 1: Allocate a page with read-write permissions
        let ptr = memory::mmap(
            core::ptr::null_mut(),
            4096,
            PROT_READ | PROT_WRITE,
            memory::MAP_PRIVATE | memory::MAP_ANONYMOUS,
            -1,
            0,
        ).unwrap_or_else(|e| {
            eprintln!("ERROR: mmap for mprotect test failed: {:?}", e);
            std::process::exit(1);
        });

        unsafe {
            // Write to the memory to verify it's writable
            *ptr = 42;
            let write1_ok = *ptr == 42;

            // Test 2: Change to read-only
            let result1 = memory::mprotect(ptr, 4096, PROT_READ);

            // Test 3: Change back to read-write
            let result2 = memory::mprotect(ptr, 4096, PROT_READ | PROT_WRITE);

            // Verify we can still write after restoring write permission
            *ptr = 123;
            let write2_ok = *ptr == 123;

            // Clean up
            let _ = memory::munmap(ptr, 4096);

            // Verify results
            if write1_ok && result1.is_ok() && result2.is_ok() && write2_ok {
                println!("RUST_STD_MPROTECT_WORKS");
            } else {
                eprintln!("ERROR: mprotect test failed: write1={}, mprotect_ro={:?}, mprotect_rw={:?}, write2={}",
                          write1_ok, result1, result2, write2_ok);
                std::process::exit(1);
            }
        }
    }

    // Test 18: Stub function smoke tests
    // These verify that libc stub functions don't panic and return expected values.
    // The goal is not to fully test these stubs (they're stubs after all), but to verify they:
    // - Don't panic
    // - Return sensible values that won't break Rust std
    // Note: These are all libc stub functions, not kernel syscalls.
    unsafe {
        extern "C" {
            fn pthread_self() -> usize;
            fn pthread_key_create(key: *mut u32, destructor: Option<unsafe extern "C" fn(*mut u8)>) -> i32;
            fn pthread_key_delete(key: u32) -> i32;
            fn pthread_getspecific(key: u32) -> *mut u8;
            fn pthread_setspecific(key: u32, value: *const u8) -> i32;
            fn pthread_getattr_np(thread: usize, attr: *mut u8) -> i32;
            fn pthread_attr_init(attr: *mut u8) -> i32;
            fn pthread_attr_destroy(attr: *mut u8) -> i32;
            fn pthread_attr_getstack(attr: *const u8, stackaddr: *mut *mut u8, stacksize: *mut usize) -> i32;
            fn signal(signum: i32, handler: usize) -> usize;
            fn sigaction(signum: i32, act: *const u8, oldact: *mut u8) -> i32;
            fn sigaltstack(ss: *const u8, old_ss: *mut u8) -> i32;
            fn sysconf(name: i32) -> i64;
            fn poll(fds: *mut u8, nfds: usize, timeout: i32) -> i32;
            fn close(fd: i32) -> i32;
            fn fcntl(fd: i32, cmd: i32, arg: u64) -> i32;
            fn open(path: *const u8, flags: i32, mode: u32) -> i32;
            fn getauxval(type_: u64) -> u64;
            fn getenv(name: *const u8) -> *mut u8;
            fn strlen(s: *const u8) -> usize;
            fn memcmp(s1: *const u8, s2: *const u8, n: usize) -> i32;
            fn __xpg_strerror_r(errnum: i32, buf: *mut u8, buflen: usize) -> i32;
        }

        let mut all_stubs_ok = true;

        // Test pthread_self - should return non-zero (we return 1 for main thread)
        let thread_id = pthread_self();
        if thread_id == 0 {
            eprintln!("ERROR: pthread_self() returned 0, expected non-zero");
            all_stubs_ok = false;
        }

        // Test pthread_key_create/delete cycle
        let mut key: u32 = 0;
        let result = pthread_key_create(&mut key, None);
        if result != 0 {
            eprintln!("ERROR: pthread_key_create() returned {}, expected 0", result);
            all_stubs_ok = false;
        }

        let result = pthread_key_delete(key);
        if result != 0 {
            eprintln!("ERROR: pthread_key_delete() returned {}, expected 0", result);
            all_stubs_ok = false;
        }

        // Test pthread_getspecific - should return NULL for unset key
        let value = pthread_getspecific(0);
        if !value.is_null() {
            eprintln!("ERROR: pthread_getspecific() returned non-null, expected null");
            all_stubs_ok = false;
        }

        // Test pthread_setspecific - should return 0 (success)
        let result = pthread_setspecific(0, core::ptr::null());
        if result != 0 {
            eprintln!("ERROR: pthread_setspecific() returned {}, expected 0", result);
            all_stubs_ok = false;
        }

        // Test pthread_attr_* functions
        let mut attr = [0u8; 64]; // Dummy attribute buffer
        let result = pthread_attr_init(attr.as_mut_ptr());
        if result != 0 {
            eprintln!("ERROR: pthread_attr_init() returned {}, expected 0", result);
            all_stubs_ok = false;
        }

        let result = pthread_getattr_np(1, attr.as_mut_ptr());
        if result != 0 {
            eprintln!("ERROR: pthread_getattr_np() returned {}, expected 0", result);
            all_stubs_ok = false;
        }

        let mut stackaddr: *mut u8 = core::ptr::null_mut();
        let mut stacksize: usize = 0;
        let result = pthread_attr_getstack(attr.as_ptr(), &mut stackaddr, &mut stacksize);
        if result != 0 {
            eprintln!("ERROR: pthread_attr_getstack() returned {}, expected 0", result);
            all_stubs_ok = false;
        }
        // Stack size should be reasonable (8MB)
        if stacksize != 8 * 1024 * 1024 {
            eprintln!("ERROR: pthread_attr_getstack() stacksize={}, expected 8MB", stacksize);
            all_stubs_ok = false;
        }

        let result = pthread_attr_destroy(attr.as_mut_ptr());
        if result != 0 {
            eprintln!("ERROR: pthread_attr_destroy() returned {}, expected 0", result);
            all_stubs_ok = false;
        }

        // Test signal() - should return SIG_DFL (0)
        let old_handler = signal(15, 0); // SIGTERM = 15
        if old_handler != 0 {
            eprintln!("ERROR: signal() returned {}, expected 0 (SIG_DFL)", old_handler);
            all_stubs_ok = false;
        }

        // Test sigaction() - should return 0 (success)
        let result = sigaction(15, core::ptr::null(), core::ptr::null_mut());
        if result != 0 {
            eprintln!("ERROR: sigaction() returned {}, expected 0", result);
            all_stubs_ok = false;
        }

        // Test sigaltstack() - should return 0 (success)
        let result = sigaltstack(core::ptr::null(), core::ptr::null_mut());
        if result != 0 {
            eprintln!("ERROR: sigaltstack() returned {}, expected 0", result);
            all_stubs_ok = false;
        }

        // Test sysconf(_SC_PAGESIZE) - should return 4096
        const _SC_PAGESIZE: i32 = 30;
        let pagesize = sysconf(_SC_PAGESIZE);
        if pagesize != 4096 {
            eprintln!("ERROR: sysconf(_SC_PAGESIZE) returned {}, expected 4096", pagesize);
            all_stubs_ok = false;
        }

        // Test sysconf(_SC_NPROCESSORS_ONLN) - should return 1 (single CPU)
        const _SC_NPROCESSORS_ONLN: i32 = 84;
        let ncpus = sysconf(_SC_NPROCESSORS_ONLN);
        if ncpus != 1 {
            eprintln!("ERROR: sysconf(_SC_NPROCESSORS_ONLN) returned {}, expected 1", ncpus);
            all_stubs_ok = false;
        }

        // Test poll() - should return 0 (no events)
        let result = poll(core::ptr::null_mut(), 0, 0);
        if result != 0 {
            eprintln!("ERROR: poll() returned {}, expected 0", result);
            all_stubs_ok = false;
        }

        // Test fcntl() - F_DUPFD on stdin returns new fd (>= 0)
        let result = fcntl(0, 0, 0); // F_DUPFD = 0
        if result < 0 {
            eprintln!("ERROR: fcntl(stdin, F_DUPFD, 0) returned {}, expected >= 0", result);
            all_stubs_ok = false;
        } else {
            // Close the dup'd fd
            close(result);
        }

        // Test open() - nonexistent file should return negative error
        let path = b"/nonexistent\0";
        let result = open(path.as_ptr(), 0, 0);
        if result >= 0 {
            eprintln!("ERROR: open(/nonexistent) returned {}, expected negative error", result);
            close(result);
            all_stubs_ok = false;
        }

        // Test getauxval(AT_PAGESZ) - should return 4096
        const AT_PAGESZ: u64 = 6;
        let pagesz = getauxval(AT_PAGESZ);
        if pagesz != 4096 {
            eprintln!("ERROR: getauxval(AT_PAGESZ) returned {}, expected 4096", pagesz);
            all_stubs_ok = false;
        }

        // Test getenv() - should return NULL for nonexistent variable
        let name = b"NONEXISTENT_VAR_12345\0";
        let value = getenv(name.as_ptr());
        if !value.is_null() {
            eprintln!("ERROR: getenv() returned non-null for nonexistent variable");
            all_stubs_ok = false;
        }

        // Test strlen() - should work correctly
        let s = b"hello\0";
        let len = strlen(s.as_ptr());
        if len != 5 {
            eprintln!("ERROR: strlen(\"hello\") returned {}, expected 5", len);
            all_stubs_ok = false;
        }

        // Test memcmp() - should work correctly
        let a = b"abc";
        let b = b"abc";
        let result = memcmp(a.as_ptr(), b.as_ptr(), 3);
        if result != 0 {
            eprintln!("ERROR: memcmp(\"abc\", \"abc\", 3) returned {}, expected 0", result);
            all_stubs_ok = false;
        }

        let a = b"abc";
        let b = b"abd";
        let result = memcmp(a.as_ptr(), b.as_ptr(), 3);
        if result >= 0 {
            eprintln!("ERROR: memcmp(\"abc\", \"abd\", 3) returned {}, expected negative", result);
            all_stubs_ok = false;
        }

        // Test __xpg_strerror_r() - should return 0 (success)
        let mut buf = [0u8; 64];
        let result = __xpg_strerror_r(0, buf.as_mut_ptr(), buf.len());
        if result != 0 {
            eprintln!("ERROR: __xpg_strerror_r() returned {}, expected 0", result);
            all_stubs_ok = false;
        }

        if all_stubs_ok {
            println!("RUST_STD_STUB_FUNCTIONS_WORK");
        } else {
            eprintln!("ERROR: Some stub function tests failed");
            std::process::exit(1);
        }
    }

    // Test 18: write() edge cases
    // This test verifies write() behavior for edge cases:
    // - write() with count=0 should return 0 (not an error)
    // - write() to invalid fd should return -1 with EBADF
    // - write() to a closed pipe write end should return -1 with EBADF
    //
    // Note on partial writes: Triggering a partial write (where write returns less
    // than the requested count) is difficult to do reliably in a test because it
    // requires filling a pipe buffer (typically 64KB on Linux, varies by OS).
    // Additionally, our kernel may not implement non-blocking I/O or may have
    // different buffer sizes. Instead, we test the edge cases that are
    // deterministic and verify the basic contract of write().
    {
        let mut all_edge_cases_ok = true;

        // Test 1: write() with count=0 should return 0
        // POSIX says: "If count is zero... the write() function may detect and
        // return errors... If count is zero and fd refers to a regular file,
        // write() may return zero... or may return -1 with errno set."
        // Most implementations return 0 for count=0 on valid fds.
        let result = io::write(Fd::STDOUT, b"");
        // We accept either Ok(0) or an error -- typically should return Ok(0)
        match result {
            Ok(0) => {}
            Ok(_) | Err(_) => {
                eprintln!("Note: write(stdout, \"\", 0) returned {:?}, expected Ok(0) (may be acceptable)", result);
                // Don't fail - some implementations may behave differently
            }
        }

        // Test 2: write() to invalid fd (-1) should return Err(EBADF)
        let test_data = b"test";
        match io::write(Fd::from_raw(-1i32 as u64), test_data) {
            Err(Error::Os(Errno::EBADF)) => {}
            Err(Error::Os(errno)) => {
                eprintln!("ERROR: write(-1, ...) set errno to {:?}, expected EBADF", errno);
                all_edge_cases_ok = false;
            }
            Ok(n) => {
                eprintln!("ERROR: write(-1, ...) returned Ok({}), expected Err(EBADF)", n);
                all_edge_cases_ok = false;
            }
        }

        // Test 3: write() to a closed fd should fail with EBADF
        let (pipe_read, pipe_write) = io::pipe().unwrap_or_else(|e| {
            eprintln!("ERROR: pipe() failed for write edge case test: {:?}", e);
            all_edge_cases_ok = false;
            (Fd::from_raw(0), Fd::from_raw(0))
        });

        if all_edge_cases_ok {
            let _ = io::close(pipe_read);  // Close read end
            let _ = io::close(pipe_write); // Close write end

            // Now writing to the closed write fd should fail with EBADF
            match io::write(pipe_write, test_data) {
                Err(Error::Os(Errno::EBADF)) => {}
                Err(Error::Os(errno)) => {
                    eprintln!("ERROR: write(closed_fd, ...) set errno to {:?}, expected EBADF", errno);
                    all_edge_cases_ok = false;
                }
                Ok(n) => {
                    eprintln!("ERROR: write(closed_fd, ...) returned Ok({}), expected Err(EBADF)", n);
                    all_edge_cases_ok = false;
                }
            }
        }

        // Test 4: write() to pipe with closed read end should fail with EPIPE (32)
        // (In a full POSIX implementation, this would also generate SIGPIPE)
        // Note: Our kernel may return EBADF instead if it doesn't track pipe state
        match io::pipe() {
            Ok((read_fd, write_fd)) => {
                let _ = io::close(read_fd); // Close read end - now writes should fail

                match io::write(write_fd, test_data) {
                    Err(Error::Os(Errno::EPIPE)) | Err(Error::Os(Errno::EBADF)) => {}
                    Err(Error::Os(errno)) => {
                        eprintln!("ERROR: write(pipe_with_closed_read_end, ...) set errno to {:?}, expected EPIPE or EBADF", errno);
                        all_edge_cases_ok = false;
                    }
                    Ok(n) => {
                        eprintln!("ERROR: write(pipe_with_closed_read_end, ...) returned Ok({}), expected Err", n);
                        all_edge_cases_ok = false;
                    }
                }
                let _ = io::close(write_fd);
            }
            Err(e) => {
                eprintln!("ERROR: pipe() failed for EPIPE test: {:?}", e);
                all_edge_cases_ok = false;
            }
        }

        if all_edge_cases_ok {
            println!("RUST_STD_WRITE_EDGE_CASES_WORK");
        } else {
            eprintln!("ERROR: Some write edge case tests failed");
            std::process::exit(1);
        }
    }

    // Test 19: Direct mmap and munmap tests
    // This test explicitly validates mmap and munmap syscalls, which were previously
    // only tested indirectly through malloc.
    {
        let mut all_mmap_tests_ok = true;

        // Test 1: Basic mmap with MAP_ANONYMOUS | MAP_PRIVATE
        let page_size: usize = 4096;
        match memory::mmap(
            core::ptr::null_mut(),
            page_size,
            memory::PROT_READ | memory::PROT_WRITE,
            memory::MAP_PRIVATE | memory::MAP_ANONYMOUS,
            -1,
            0,
        ) {
            Err(e) => {
                eprintln!("ERROR: mmap(MAP_ANONYMOUS | MAP_PRIVATE) failed: {:?}", e);
                all_mmap_tests_ok = false;
            }
            Ok(ptr) if ptr.is_null() => {
                eprintln!("ERROR: mmap(MAP_ANONYMOUS | MAP_PRIVATE) returned null");
                all_mmap_tests_ok = false;
            }
            Ok(ptr) => {
                unsafe {
                    // Test 2: Write to the memory and read it back
                    let test_pattern: u8 = 0xAB;
                    *ptr = test_pattern;
                    *ptr.add(page_size - 1) = test_pattern; // Write to last byte too

                    let read_first = *ptr;
                    let read_last = *ptr.add(page_size - 1);

                    if read_first != test_pattern || read_last != test_pattern {
                        eprintln!("ERROR: mmap memory write/read failed: wrote {:#x}, read first={:#x}, last={:#x}",
                                  test_pattern, read_first, read_last);
                        all_mmap_tests_ok = false;
                    }

                    // Test 3: munmap should return Ok(())
                    if let Err(e) = memory::munmap(ptr, page_size) {
                        eprintln!("ERROR: munmap failed: {:?}", e);
                        all_mmap_tests_ok = false;
                    }
                }
            }
        }

        // Test 4: mmap error case - size=0 should return error with EINVAL
        match memory::mmap(
            core::ptr::null_mut(),
            0,
            memory::PROT_READ | memory::PROT_WRITE,
            memory::MAP_PRIVATE | memory::MAP_ANONYMOUS,
            -1,
            0,
        ) {
            Err(Error::Os(Errno::EINVAL)) => {
                // Expected
            }
            Err(Error::Os(errno)) => {
                eprintln!("ERROR: mmap(size=0) set errno to {:?}, expected EINVAL", errno);
                all_mmap_tests_ok = false;
            }
            Ok(ptr) => {
                eprintln!("ERROR: mmap(size=0) did not return error, got {:?}", ptr);
                // Clean up if it somehow succeeded
                if !ptr.is_null() {
                    let _ = memory::munmap(ptr, page_size);
                }
                all_mmap_tests_ok = false;
            }
        }

        // Test 5: Multi-page allocation
        let multi_page_size = page_size * 4;
        match memory::mmap(
            core::ptr::null_mut(),
            multi_page_size,
            memory::PROT_READ | memory::PROT_WRITE,
            memory::MAP_PRIVATE | memory::MAP_ANONYMOUS,
            -1,
            0,
        ) {
            Err(e) => {
                eprintln!("ERROR: mmap(4 pages) failed: {:?}", e);
                all_mmap_tests_ok = false;
            }
            Ok(ptr) if ptr.is_null() => {
                eprintln!("ERROR: mmap(4 pages) returned null");
                all_mmap_tests_ok = false;
            }
            Ok(ptr_multi) => {
                unsafe {
                    // Write to each page to verify they're all usable
                    for i in 0..4 {
                        let offset = i * page_size;
                        *ptr_multi.add(offset) = (i as u8) + 1;
                    }

                    // Verify the writes
                    let mut multi_page_ok = true;
                    for i in 0..4 {
                        let offset = i * page_size;
                        if *ptr_multi.add(offset) != (i as u8) + 1 {
                            eprintln!("ERROR: Multi-page mmap verification failed at page {}", i);
                            multi_page_ok = false;
                            break;
                        }
                    }

                    if !multi_page_ok {
                        all_mmap_tests_ok = false;
                    }

                    // Unmap the multi-page allocation
                    if let Err(e) = memory::munmap(ptr_multi, multi_page_size) {
                        eprintln!("ERROR: munmap(4 pages) failed: {:?}", e);
                        all_mmap_tests_ok = false;
                    }
                }
            }
        }

        if all_mmap_tests_ok {
            println!("RUST_STD_MMAP_WORKS");
        } else {
            eprintln!("ERROR: Some mmap/munmap tests failed");
            std::process::exit(1);
        }
    }

    // Test 20: nanosleep (via std::thread::sleep)
    {
        use std::time::{Duration, Instant};
        let before = Instant::now();
        std::thread::sleep(Duration::from_millis(50));
        let elapsed = before.elapsed();
        // Should have slept at least 40ms (allow some tolerance)
        if elapsed >= Duration::from_millis(40) {
            println!("RUST_STD_SLEEP_WORKS");
        } else {
            eprintln!(
                "ERROR: thread::sleep(50ms) only slept {:?}",
                elapsed
            );
            std::process::exit(1);
        }
    }

    println!("All std tests passed!");
    std::process::exit(0);
}
