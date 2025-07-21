#![no_std]
#![no_main]

use core::panic::PanicInfo;

// System call numbers
const SYS_EXIT: u64 = 0;
const SYS_WRITE: u64 = 1;
const SYS_YIELD: u64 = 3;
const SYS_GET_TIME: u64 = 4;

// Simple syscall wrappers
unsafe fn syscall0(n: u64) -> u64 {
    let ret: u64;
    core::arch::asm!(
        "int 0x80",
        in("rax") n,
        lateout("rax") ret,
        options(nostack, preserves_flags)
    );
    ret
}

unsafe fn syscall1(n: u64, arg1: u64) -> u64 {
    let ret: u64;
    core::arch::asm!(
        "int 0x80",
        in("rax") n,
        in("rdi") arg1,
        lateout("rax") ret,
        options(nostack, preserves_flags)
    );
    ret
}

unsafe fn syscall3(n: u64, arg1: u64, arg2: u64, arg3: u64) -> u64 {
    let ret: u64;
    core::arch::asm!(
        "int 0x80",
        in("rax") n,
        in("rdi") arg1,
        in("rsi") arg2,
        in("rdx") arg3,
        lateout("rax") ret,
        options(nostack, preserves_flags)
    );
    ret
}

// Helper to write a string
fn write_str(s: &str) {
    unsafe {
        syscall3(SYS_WRITE, 1, s.as_ptr() as u64, s.len() as u64);
    }
}

// Helper to write a number
fn write_num(mut n: u64) {
    if n == 0 {
        write_str("0");
        return;
    }
    
    let mut buf = [0u8; 20];
    let mut i = 19;
    
    while n > 0 {
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
        i -= 1;
    }
    
    let s = unsafe { core::str::from_utf8_unchecked(&buf[i + 1..20]) };
    write_str(s);
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    write_str("=== Timer Test Program ===\n");
    
    // Test 1: Get initial time
    write_str("Test 1: Initial time = ");
    let time1 = unsafe { syscall0(SYS_GET_TIME) };
    write_num(time1);
    write_str(" ms\n");
    
    // Test 2: Yield 10 times and check time
    write_str("Test 2: Yielding 10 times...\n");
    for _ in 0..10 {
        unsafe { syscall0(SYS_YIELD); }
    }
    
    let time2 = unsafe { syscall0(SYS_GET_TIME) };
    write_str("        Time after yields = ");
    write_num(time2);
    write_str(" ms (delta = ");
    write_num(time2.saturating_sub(time1));
    write_str(" ms)\n");
    
    // Test 3: Busy wait and check time
    write_str("Test 3: Busy waiting ~100ms...\n");
    for _ in 0..10_000_000 {
        // Busy wait
        unsafe { core::arch::asm!("nop") };
    }
    
    let time3 = unsafe { syscall0(SYS_GET_TIME) };
    write_str("        Time after busy wait = ");
    write_num(time3);
    write_str(" ms (delta = ");
    write_num(time3.saturating_sub(time2));
    write_str(" ms)\n");
    
    // Test 4: Multiple rapid time calls
    write_str("Test 4: Rapid time calls:\n");
    for i in 0..5 {
        let t = unsafe { syscall0(SYS_GET_TIME) };
        write_str("        Call ");
        write_num(i as u64);
        write_str(": ");
        write_num(t);
        write_str(" ms\n");
    }
    
    // Test 5: Long wait with progress
    write_str("Test 5: Waiting 1 second with progress...\n");
    let start_time = unsafe { syscall0(SYS_GET_TIME) };
    
    for i in 0..10 {
        // Wait ~100ms
        for _ in 0..100 {
            unsafe { syscall0(SYS_YIELD); }
        }
        
        let current = unsafe { syscall0(SYS_GET_TIME) };
        write_str("        ");
        write_num(i as u64 + 1);
        write_str("00ms: time = ");
        write_num(current);
        write_str(" (elapsed = ");
        write_num(current.saturating_sub(start_time));
        write_str(" ms)\n");
    }
    
    // Final summary
    write_str("\n=== Test Complete ===\n");
    let final_time = unsafe { syscall0(SYS_GET_TIME) };
    write_str("Total elapsed time: ");
    write_num(final_time.saturating_sub(time1));
    write_str(" ms\n");
    
    if final_time == time1 {
        write_str("ERROR: Timer is not incrementing!\n");
    } else {
        write_str("SUCCESS: Timer is working!\n");
    }
    
    // Exit
    unsafe { syscall1(SYS_EXIT, 0); }
    
    loop {}
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    write_str("PANIC in timer test!\n");
    unsafe { syscall1(SYS_EXIT, 1); }
    loop {}
}