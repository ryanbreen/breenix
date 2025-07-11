//! Test program for the spawn system call
//!
//! This program tests the spawn() syscall which creates a new process
//! directly from an ELF binary (combining fork+exec).

#![no_std]
#![no_main]

use core::panic::PanicInfo;

mod libbreenix;
use libbreenix::{sys_write, sys_spawn, sys_exit};

const STDOUT: u64 = 1;

// Test parameters
const NUM_SPAWNS: usize = 3;

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // Print initial message
    let msg = b"Spawn test starting...\n";
    unsafe { sys_write(STDOUT, msg); }
    
    // Spawn multiple processes
    for i in 0..NUM_SPAWNS {
        // Show which spawn we're doing
        let msg = format_string(b"Spawning process ", i as u64 + 1, b"...\n");
        unsafe { sys_write(STDOUT, &msg); }
        
        // Call spawn (path is ignored for now, uses hello_time.elf)
        let spawn_result = unsafe { sys_spawn("/test/hello_time", "") };
        
        // Check result
        if spawn_result as i64 > 0 {
            // Success - got a PID back
            let msg = format_string(b"Spawned process ", spawn_result, b"\n");
            unsafe { sys_write(STDOUT, &msg); }
        } else {
            // Error
            let msg = b"Spawn failed!\n";
            unsafe { sys_write(STDOUT, msg); }
        }
        
        // Small delay between spawns
        for _ in 0..1000000 {
            unsafe { core::arch::asm!("nop"); }
        }
    }
    
    // Wait a bit to let spawned processes run
    let msg = b"Waiting for spawned processes to complete...\n";
    unsafe { sys_write(STDOUT, msg); }
    
    for _ in 0..10000000 {
        unsafe { core::arch::asm!("nop"); }
    }
    
    // Exit
    let msg = b"Spawn test complete!\n";
    unsafe { sys_write(STDOUT, msg); }
    unsafe { sys_exit(0); }
}

// Simple number to string conversion
fn format_string(prefix: &[u8], num: u64, suffix: &[u8]) -> [u8; 64] {
    let mut buffer = [0u8; 64];
    let mut pos = 0;
    
    // Copy prefix
    for &b in prefix {
        if pos < buffer.len() {
            buffer[pos] = b;
            pos += 1;
        }
    }
    
    // Convert number to string
    if num == 0 {
        if pos < buffer.len() {
            buffer[pos] = b'0';
            pos += 1;
        }
    } else {
        let mut n = num;
        let mut digits = [0u8; 20];
        let mut digit_count = 0;
        
        while n > 0 {
            digits[digit_count] = (n % 10) as u8 + b'0';
            digit_count += 1;
            n /= 10;
        }
        
        // Copy digits in reverse order
        for i in (0..digit_count).rev() {
            if pos < buffer.len() {
                buffer[pos] = digits[i];
                pos += 1;
            }
        }
    }
    
    // Copy suffix
    for &b in suffix {
        if pos < buffer.len() {
            buffer[pos] = b;
            pos += 1;
        }
    }
    
    buffer
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    // Write panic message
    let msg = b"Spawn test panicked!\n";
    unsafe { sys_write(STDOUT, msg); }
    unsafe { sys_exit(1); }
}