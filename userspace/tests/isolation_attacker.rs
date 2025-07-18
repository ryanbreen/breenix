#![no_std]
#![no_main]

mod libbreenix;
use libbreenix::{sys_get_shared_test_page, sys_write, sys_exit};

#[no_mangle]
pub extern "C" fn _start() -> ! {
    unsafe {
        sys_write(1, b"[ATTACKER] Starting isolation attack test\n");
        
        // Get the victim's shared test address
        let victim_addr = sys_get_shared_test_page();
        
        sys_write(1, b"[ATTACKER] Got victim address: 0x");
        print_hex(victim_addr);
        sys_write(1, b"\n");
        
        // Try to read from the victim's page - this SHOULD cause a page fault
        sys_write(1, b"[ATTACKER] Attempting to read from victim's page...\n");
        
        // This should trigger a page fault and kill this process
        let value = core::ptr::read_volatile(victim_addr as *const u8);
        
        // If we get here, isolation has FAILED!
        sys_write(1, b"*** SECURITY BUG: read succeeded! Value = ");
        print_number(value as u64);
        sys_write(1, b" ***\n");
        sys_exit(1);
    }
}

/// Simple number to string conversion for printing
unsafe fn print_number(mut n: u64) {
    if n == 0 {
        sys_write(1, b"0");
        return;
    }
    
    let mut buffer = [0u8; 20];
    let mut i = 0;
    
    while n > 0 {
        buffer[i] = b'0' + (n % 10) as u8;
        n /= 10;
        i += 1;
    }
    
    // Print in reverse order
    while i > 0 {
        i -= 1;
        sys_write(1, &buffer[i..i+1]);
    }
}

/// Print a number in hex
unsafe fn print_hex(mut n: u64) {
    let hex_chars = b"0123456789abcdef";
    let mut buffer = [0u8; 16];
    let mut i = 0;
    
    if n == 0 {
        sys_write(1, b"0");
        return;
    }
    
    while n > 0 {
        buffer[i] = hex_chars[(n & 0xf) as usize];
        n >>= 4;
        i += 1;
    }
    
    // Print in reverse order
    while i > 0 {
        i -= 1;
        sys_write(1, &buffer[i..i+1]);
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    unsafe {
        sys_write(1, b"[ATTACKER] PANIC!\n");
    }
    loop {}
}