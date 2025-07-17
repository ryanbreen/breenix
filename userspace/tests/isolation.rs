#![no_std]
#![no_main]

// Import the minimal userspace runtime
#[path = "libbreenix.rs"]
mod libbreenix;

use libbreenix::{sys_getpid, sys_yield, sys_exit};
use core::panic::PanicInfo;

// Test-only syscalls
const SYS_SHARE_TEST_PAGE: u64 = 100;

const PAGE_SIZE: usize = 4096;
const MARKER_VALUE: u8 = 0xA5;

// Syscall wrapper for sharing test page
unsafe fn sys_share_test_page(addr: usize) {
    core::arch::asm!(
        "mov rax, {syscall_num}",
        "mov rdi, {addr}",
        "int 0x80",
        syscall_num = const SYS_SHARE_TEST_PAGE,
        addr = in(reg) addr,
        out("rax") _,
        out("rdi") _,
        out("rcx") _,
        out("r11") _,
    );
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    unsafe {
        let pid = sys_getpid();
        
        // Write initial message
        let msg = b"[ISOLATION] Victim process started, PID=";
        libbreenix::sys_write(1, msg);
        
        // Print PID (simple single digit for now)
        let pid_char = (b'0' + (pid as u8 % 10)) as u8;
        libbreenix::sys_write(1, core::slice::from_ref(&pid_char));
        libbreenix::sys_write(1, b"\n");
        
        // 1. Use a known memory location in this process's address space
        // We'll use a location on the stack (which should be private to this process)
        let mut test_data = [MARKER_VALUE; PAGE_SIZE];
        let page = test_data.as_ptr() as usize;
        
        let msg2 = b"[ISOLATION] Allocated page at 0x";
        libbreenix::sys_write(1, msg2);
        
        // Print address in hex (simplified)
        for i in (0..16).rev() {
            let nibble = ((page >> (i * 4)) & 0xF) as u8;
            let hex_char = if nibble < 10 {
                b'0' + nibble
            } else {
                b'a' + (nibble - 10)
            };
            libbreenix::sys_write(1, core::slice::from_ref(&hex_char));
        }
        libbreenix::sys_write(1, b"\n");
        
        // 2. Data is already filled with marker value (initialized above)
        
        let msg3 = b"[ISOLATION] Filled page with marker value 0xA5\n";
        libbreenix::sys_write(1, msg3);
        
        // 3. Share the page address via syscall
        sys_share_test_page(page);
        
        let msg4 = b"[ISOLATION] Shared page address with kernel\n";
        libbreenix::sys_write(1, msg4);
        
        // 4. Loop forever, yielding
        let msg5 = b"[ISOLATION] Entering yield loop...\n";
        libbreenix::sys_write(1, msg5);
        
        loop {
            sys_yield();
        }
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    unsafe {
        sys_exit(1);
    }
}