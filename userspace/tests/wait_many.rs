//! Test wait() with multiple children
//! 
//! Parent forks five children; each _exit(i).
//! Parent loops: while ((pid = wait(&st)) > 0) collect.
//! Assert sums of statuses == 1+2+3+4+5, and order is arbitrary.

#![no_std]
#![no_main]

include!("libbreenix.rs");

// Simple print function
unsafe fn print(s: &str) {
    sys_write(1, s.as_bytes());
}

// Simple integer to string conversion
fn itoa(mut n: u32, buf: &mut [u8]) -> &str {
    if n == 0 {
        buf[0] = b'0';
        return core::str::from_utf8(&buf[..1]).unwrap();
    }
    
    let mut i = 0;
    while n > 0 {
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
        i += 1;
    }
    
    // Reverse the string
    buf[..i].reverse();
    core::str::from_utf8(&buf[..i]).unwrap()
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    unsafe {
        print("wait_many test starting\n");
        
        let mut children = [0i64; 5];
        let mut buf = [0u8; 32];
        
        // Fork 5 children
        for i in 0..5 {
            let pid = sys_fork();
            if pid == 0 {
                // Child process - exit with status i+1
                let exit_code = (i + 1) as i32;
                print("Child exiting with code ");
                print(itoa(exit_code as u32, &mut buf));
                print("\n");
                sys_exit(exit_code);
            } else if pid > 0 {
                // Parent - save child PID
                children[i] = pid as i64;
                print("Forked child ");
                print(itoa(pid as u32, &mut buf));
                print("\n");
            } else {
                print("Fork failed!\n");
                sys_exit(1);
            }
        }
        
        // Parent waits for all children
        print("Parent waiting for children...\n");
        
        let mut status_sum = 0u32;
        let mut collected_count = 0;
        
        loop {
            let mut status: u32 = 0;
            let pid = wait(&mut status as *mut u32 as *mut i32);
            
            if pid > 0 {
                collected_count += 1;
                status_sum += status & 0xFF; // Get exit status byte
                
                print("Collected child ");
                print(itoa(pid as u32, &mut buf));
                print(" with status ");
                print(itoa(status & 0xFF, &mut buf));
                print("\n");
            } else if pid < 0 {
                // No more children
                break;
            }
        }
        
        // Verify we collected all 5 children
        if collected_count != 5 {
            print("ERROR: Expected 5 children, collected ");
            print(itoa(collected_count, &mut buf));
            print("\n");
            sys_exit(1);
        }
        
        // Verify sum of statuses is 1+2+3+4+5 = 15
        if status_sum != 15 {
            print("ERROR: Expected status sum 15, got ");
            print(itoa(status_sum, &mut buf));
            print("\n");
            sys_exit(1);
        }
        
        print("✓ wait_many test passed!\n");
        sys_exit(0);
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    unsafe {
        print("wait_many test panicked!\n");
        sys_exit(1);
    }
}