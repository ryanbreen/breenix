//! Fork progress test - verifies child can execute instructions after fork
//!
//! This test ensures that the child process can make progress and execute
//! multiple instructions after fork(), not just get stuck at the first instruction.

#![no_std]
#![no_main]

include!("libbreenix.rs");

// Global counter that child will increment
static mut COUNTER: u32 = 0;

// Simple print function
unsafe fn print(s: &str) {
    sys_write(1, s.as_bytes());
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    unsafe {
        print("Fork progress test starting\n");
        
        // Fork a child
        let pid = sys_fork() as i64;
        if pid == 0 {
            // Child process - increment counter 10 times
            print("Child: Starting counter increments\n");
            for _i in 0..10 {
                COUNTER += 1;
                // Add some computation to ensure multiple instructions execute
                let temp = COUNTER * 2;
                if temp > 20 {
                    print("Child: Counter passed 10\n");
                }
            }
            print("Child: Completed 10 increments\n");
            print("Child: Exiting with status 0\n");
            sys_exit(0);
        } else if pid > 0 {
            // Parent process
            print("Parent: Forked child, waiting...\n");
            
            let mut status: u32 = 0;
            let child_pid = wait(&mut status as *mut u32 as *mut i32);
            
            if child_pid == pid {
                print("Parent: Child exited\n");
                
                // Check counter value
                if COUNTER == 10 {
                    print("✓ SUCCESS: Counter is 10 - child executed successfully!\n");
                } else {
                    print("✗ FAILURE: Counter is ");
                    // Print counter value for debugging
                    let mut buf = [0u8; 10];
                    let mut val = COUNTER;
                    let mut idx = 0;
                    while val > 0 && idx < 10 {
                        buf[idx] = b'0' + (val % 10) as u8;
                        val /= 10;
                        idx += 1;
                    }
                    if idx == 0 {
                        sys_write(1, b"0");
                    } else {
                        // Reverse the buffer
                        for i in 0..idx/2 {
                            let tmp = buf[i];
                            buf[i] = buf[idx-1-i];
                            buf[idx-1-i] = tmp;
                        }
                        sys_write(1, &buf[0..idx]);
                    }
                    print(" - child did not complete execution\n");
                }
            } else {
                print("✗ Parent: wait() returned wrong PID\n");
            }
            
            print("Fork progress test completed\n");
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
        print("Fork progress test panicked!\n");
        sys_exit(1);
    }
}