#![no_std]
#![no_main]

mod libbreenix;
use libbreenix::{sys_getpid, sys_yield, sys_share_test_page, sys_write};

// Static page-aligned buffer that we'll use as our "allocated" page
#[repr(align(4096))]
struct PageBuffer {
    data: [u8; 4096],
}

static mut TEST_PAGE: PageBuffer = PageBuffer { data: [0; 4096] };

#[no_mangle]
pub extern "C" fn _start() -> ! {
    unsafe {
        let pid = sys_getpid();
        
        // Print our PID
        let msg = b"[ISOLATION] Process started with PID: ";
        sys_write(1, msg);
        print_number(pid);
        sys_write(1, b"\n");
        
        // Get the address of our test page
        let page_addr = &mut TEST_PAGE as *mut PageBuffer as u64;
        
        // Fill it with a marker value (0xA5)
        for i in 0..4096 {
            TEST_PAGE.data[i] = 0xA5;
        }
        
        sys_write(1, b"[ISOLATION] Filled test page at address: 0x");
        print_hex(page_addr);
        sys_write(1, b"\n");
        
        // Share the page address with the kernel
        sys_share_test_page(page_addr);
        sys_write(1, b"[ISOLATION] Shared test page with kernel\n");
        
        // Loop forever, periodically yielding
        loop {
            sys_yield();
        }
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
        sys_write(1, b"[ISOLATION] PANIC!\n");
    }
    loop {}
}