//! Simple wait test to verify basic functionality

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
        print("Simple wait test starting\n");
        
        // Fork a child
        let pid = sys_fork() as i64;
        if pid == 0 {
            // Child process
            print("Child: Hello from child!\n");
            print("Child: Exiting with status 42\n");
            sys_exit(42);
        } else if pid > 0 {
            // Parent process
            print("Parent: Forked child, waiting...\n");
            
            let mut status: u32 = 0;
            let child_pid = wait(&mut status as *mut u32 as *mut i32);
            
            if child_pid == pid {
                print("Parent: Child exited successfully!\n");
                if (status & 0xFF) == 42 {
                    print("✓ Parent: Got correct exit status 42\n");
                } else {
                    print("✗ Parent: Wrong exit status\n");
                }
            } else {
                print("✗ Parent: wait() returned wrong PID\n");
            }
            
            print("Simple wait test completed\n");
            sys_exit(0);
        } else {
            print("Fork failed!\n");
            sys_exit(1);
        }
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    unsafe {
        print("Simple wait test panicked!\n");
        sys_exit(1);
    }
}