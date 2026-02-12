//! brk syscall test program (std version)
//!
//! Tests the POSIX-compliant brk() syscall for heap management.

use libbreenix::memory;
use std::process;
use std::ptr::{read_volatile, write_volatile};

fn fail(msg: &str) -> ! {
    println!("USERSPACE BRK: FAIL - {}", msg);
    process::exit(1);
}

fn main() {
    println!("=== brk Test Program ===");

    // Phase 1: Query current program break
    println!("Phase 1: Querying initial program break with sbrk(0)...");
    let initial_brk = memory::get_brk() as usize;

    println!("  Initial break: {:#018x}", initial_brk);

    // Validate initial break is in expected range
    if initial_brk < 0x40000000 || initial_brk > 0x80000000 {
        fail("Initial break outside expected range");
    }
    println!("  Initial break is valid");

    // Phase 2: Expand heap by 4KB
    println!("Phase 2: Expanding heap by 4KB...");
    let new_brk_requested = initial_brk + 4096;
    println!("  Requesting break at: {:#018x}", new_brk_requested);

    let new_brk = memory::brk(new_brk_requested as u64) as usize;
    println!("  Returned break: {:#018x}", new_brk);

    if new_brk < new_brk_requested {
        fail("Heap expansion failed");
    }
    println!("  Heap expanded successfully");

    // Phase 3: Write full page with unique patterns (512 u64 values = 4096 bytes)
    println!("Phase 3: Writing 512 unique patterns (4KB) to allocated memory...");
    let base_addr = initial_brk as *mut u64;
    const NUM_VALUES: usize = 512;

    for i in 0..NUM_VALUES {
        let pattern: u64 = 0xDEADBEEF_C0FFEE00 + (i as u64);
        unsafe {
            write_volatile(base_addr.add(i), pattern);
        }
    }
    println!("  Written 512 unique patterns");

    // Phase 4: Read back and verify all patterns
    println!("Phase 4: Verifying all 512 patterns...");
    let mut errors = 0u64;
    for i in 0..NUM_VALUES {
        let expected: u64 = 0xDEADBEEF_C0FFEE00 + (i as u64);
        let actual = unsafe { read_volatile(base_addr.add(i)) };
        if actual != expected {
            errors += 1;
            if errors <= 3 {
                println!("  ERROR at offset {:#x}: expected {:#018x}, got {:#018x}", i, expected, actual);
            }
        }
    }

    if errors > 0 {
        println!("  FAIL: {} verification errors", errors);
        fail("Full page memory verification failed");
    }
    println!("  All 512 patterns verified successfully");

    // Phase 5: Expand again and test second region
    println!("Phase 5: Expanding by another 4KB and testing...");
    let second_brk_requested = new_brk + 4096;
    let second_brk = memory::brk(second_brk_requested as u64) as usize;

    if second_brk < second_brk_requested {
        fail("Second heap expansion failed");
    }

    // Write to second region
    let second_test_addr = new_brk as *mut u64;
    let second_pattern: u64 = 0x12345678_9abcdef0;

    unsafe {
        write_volatile(second_test_addr, second_pattern);
    }

    let second_read = unsafe { read_volatile(second_test_addr) };
    if second_read != second_pattern {
        fail("Second region memory verification failed");
    }
    println!("  Second region verified successfully");

    // Phase 6: Contract heap back to initial size
    println!("Phase 6: Contracting heap back to initial size...");
    println!("  Current break: {:#018x}", second_brk);
    println!("  Requesting: {:#018x}", initial_brk);

    let contracted_brk = memory::brk(initial_brk as u64) as usize;
    println!("  Returned break: {:#018x}", contracted_brk);

    if contracted_brk != initial_brk {
        println!("  FAIL: Expected break at {:#018x} but got {:#018x}", initial_brk, contracted_brk);
        fail("Heap contraction failed");
    }
    println!("  Heap contracted successfully to initial size");

    // Phase 7: Verify we can expand again after contraction
    println!("Phase 7: Re-expanding heap after contraction...");
    let reexpand_brk = memory::brk((initial_brk + 4096) as u64) as usize;

    if reexpand_brk < initial_brk + 4096 {
        fail("Re-expansion after contraction failed");
    }

    println!("  Re-expand brk returned: {:#018x}", reexpand_brk);
    println!("  Writing to addr: {:#018x}", initial_brk);

    let reexpand_addr = initial_brk as *mut u64;
    let reexpand_pattern: u64 = 0xCAFEBABE_DEADBEEF;

    unsafe {
        write_volatile(reexpand_addr, reexpand_pattern);
    }
    println!("  Pattern written");

    let reexpand_read = unsafe { read_volatile(reexpand_addr) };
    println!("  Read back: {:#018x}", reexpand_read);
    println!("  Expected: {:#018x}", reexpand_pattern);

    if reexpand_read != reexpand_pattern {
        println!("  MISMATCH!");
        fail("Re-expanded region memory verification failed");
    }
    println!("  Re-expansion verified successfully");

    // All tests passed
    println!("USERSPACE BRK: ALL TESTS PASSED");
    process::exit(0);
}
