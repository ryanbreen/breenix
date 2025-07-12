//! Test ECHILD error when no children exist
//! 
//! Parent with no children calls wait(&st) → returns −1/ECHILD.

#![no_std]
#![no_main]

include!("libbreenix.rs");

// Simple print function
unsafe fn print(s: &str) {
    sys_write(1, s.as_bytes());
}

// Simple integer to string conversion
fn itoa(mut n: i32, buf: &mut [u8]) -> &str {
    let negative = n < 0;
    if negative {
        n = -n;
    }
    
    if n == 0 {
        buf[0] = b'0';
        return core::str::from_utf8(&buf[..1]).unwrap();
    }
    
    let mut i = 0;
    let mut un = n as u32;
    while un > 0 {
        buf[i] = b'0' + (un % 10) as u8;
        un /= 10;
        i += 1;
    }
    
    if negative {
        buf[i] = b'-';
        i += 1;
    }
    
    // Reverse the string
    buf[..i].reverse();
    core::str::from_utf8(&buf[..i]).unwrap()
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    unsafe {
        print("echld_error test starting\n");
        
        let mut buf = [0u8; 32];
        
        // Try to wait when we have no children
        print("Calling wait() with no children...\n");
        
        let mut status: u32 = 0;
        let result = wait(&mut status as *mut u32 as *mut i32);
        
        print("wait() returned: ");
        print(itoa(result as i32, &mut buf));
        print("\n");
        
        // Should return -ECHILD (-10)
        if result != -10 {
            print("ERROR: Expected -10 (ECHILD), got ");
            print(itoa(result as i32, &mut buf));
            print("\n");
            sys_exit(1);
        }
        
        print("✓ Got ECHILD error as expected\n");
        
        // Also test waitpid with no children
        print("Calling waitpid(-1, ...) with no children...\n");
        
        let result2 = waitpid(-1, &mut status as *mut u32 as *mut i32, 0);
        
        print("waitpid() returned: ");
        print(itoa(result2 as i32, &mut buf));
        print("\n");
        
        if result2 != -10 {
            print("ERROR: Expected -10 (ECHILD), got ");
            print(itoa(result2 as i32, &mut buf));
            print("\n");
            sys_exit(1);
        }
        
        print("✓ Got ECHILD error from waitpid as expected\n");
        print("✓ echld_error test passed!\n");
        sys_exit(0);
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    unsafe {
        print("echld_error test panicked!\n");
        sys_exit(1);
    }
}