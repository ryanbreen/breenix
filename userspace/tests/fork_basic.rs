//! Basic fork test - parent prints "P", child prints "C"

#![no_std]
#![no_main]

include!("libbreenix.rs");

// Simple print function
unsafe fn print(s: &str) {
    sys_write(1, s.as_bytes());
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    unsafe {
        print("fork_basic: Starting test\n");
        
        // Fork a child
        let pid = sys_fork() as i64;
        
        if pid < 0 {
            print("fork_basic: Fork failed!\n");
            sys_exit(1);
        } else if pid == 0 {
            // Child process
            print("C");
            print("\n");
            print("fork_basic: Child exiting\n");
            sys_exit(0);
        } else {
            // Parent process
            print("P");
            print("\n");
            print("fork_basic: Parent waiting for child\n");
            
            let mut status: u32 = 0;
            let wait_result = wait(&mut status as *mut u32 as *mut i32);
            
            if wait_result == pid {
                print("fork_basic: Child exited successfully\n");
                print("✓ fork_basic: Test passed\n");
                sys_exit(0);
            } else {
                print("✗ fork_basic: Wait failed\n");
                sys_exit(1);
            }
        }
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    unsafe {
        print("fork_basic: Panic!\n");
        sys_exit(1);
    }
}