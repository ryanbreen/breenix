#![no_std]
#![no_main]

mod libbreenix;
use libbreenix::{sys_share_test_page, sys_get_shared_test_page, sys_exit};

#[no_mangle]
pub extern "C" fn _start() -> ! {
    unsafe {
        // Test round-trip with a recognizable value - no string output to avoid .rodata issue
        let test_value = 0xdead_beef;
        
        // Call syscall 400
        sys_share_test_page(test_value);
        
        // Call syscall 401
        let result = sys_get_shared_test_page();
        
        // Compare in register and exit with appropriate code
        if result == test_value {
            sys_exit(0); // Success
        } else {
            sys_exit(1); // Failure
        }
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    unsafe {
        sys_exit(1); // Exit with error code on panic
    }
}