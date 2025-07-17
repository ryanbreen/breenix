#![no_std]
#![no_main]

// Import the minimal userspace runtime
#[path = "libbreenix.rs"]
mod libbreenix;

use libbreenix::{sys_exit};
use core::panic::PanicInfo;

// Test-only syscalls
const SYS_GET_SHARED_TEST_PAGE: u64 = 101;

// Syscall wrapper for getting shared test page
unsafe fn sys_get_shared_test_page() -> usize {
    let mut result: usize;
    core::arch::asm!(
        "mov rax, {syscall_num}",
        "int 0x80",
        "mov {result}, rax",
        syscall_num = const SYS_GET_SHARED_TEST_PAGE,
        result = out(reg) result,
        out("rax") _,
        out("rcx") _,
        out("r11") _,
    );
    result
}

fn write_const(msg: &[u8]) {
    unsafe {
        libbreenix::sys_write(1, msg);
    }
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    unsafe {
        write_const(b"[ATTACKER] Process started\n");
        
        // 1. Ask kernel for the victim's shared test address
        let victim_addr = sys_get_shared_test_page();
        
        if victim_addr == 0 {
            write_const(b"[ATTACKER] No shared page available yet\n");
            sys_exit(2);
        }
        
        write_const(b"[ATTACKER] Got victim address: 0x");
        
        // Print address in hex (simplified)
        for i in (0..16).rev() {
            let nibble = ((victim_addr >> (i * 4)) & 0xF) as u8;
            let hex_char = if nibble < 10 {
                b'0' + nibble
            } else {
                b'a' + (nibble - 10)
            };
            libbreenix::sys_write(1, core::slice::from_ref(&hex_char));
        }
        write_const(b"\n");
        
        write_const(b"[ATTACKER] Attempting to read victim's page...\n");
        
        // 2. TRY READ — should trigger page fault
        let value = core::ptr::read_volatile(victim_addr as *const u8);
        
        // 3. If we got here, isolation FAILED!
        write_const(b"*** SECURITY BUG: read succeeded! Value = 0x");
        
        // Print the value we read
        let high = (value >> 4) & 0xF;
        let low = value & 0xF;
        let high_char = if high < 10 { b'0' + high } else { b'a' + (high - 10) };
        let low_char = if low < 10 { b'0' + low } else { b'a' + (low - 10) };
        libbreenix::sys_write(1, core::slice::from_ref(&high_char));
        libbreenix::sys_write(1, core::slice::from_ref(&low_char));
        
        write_const(b" ***\n");
        sys_exit(1);
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    unsafe {
        sys_exit(1);
    }
}