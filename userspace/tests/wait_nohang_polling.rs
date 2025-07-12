//! Test waitpid() with WNOHANG (non-blocking)
//! 
//! Fork child that sleeps 100 ticks, then _exit(0).
//! Parent spins with waitpid(child, &st, WNOHANG); expects 0 until child exits, then child PID.

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

// Simple sleep function
unsafe fn sleep_ticks(ticks: u64) {
    let start = sys_get_time();
    while sys_get_time() - start < ticks {
        sys_yield();
    }
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    unsafe {
        print("wait_nohang_polling test starting\n");
        
        let mut buf = [0u8; 32];
        
        // Fork child
        let child_pid = sys_fork() as i64;
        if child_pid == 0 {
            // Child - sleep 100 ticks then exit
            print("Child sleeping for 100 ticks...\n");
            sleep_ticks(100);
            print("Child done sleeping, exiting with code 42\n");
            sys_exit(42);
        } else if child_pid < 0 {
            print("Fork failed!\n");
            sys_exit(1);
        }
        
        print("Forked child with PID ");
        print(itoa(child_pid as u32, &mut buf));
        print("\n");
        
        // Parent polls with WNOHANG
        print("Parent polling with WNOHANG...\n");
        
        let mut poll_count = 0u32;
        let mut got_zero = false;
        
        loop {
            let mut status: u32 = 0;
            let result = waitpid(child_pid, &mut status as *mut u32 as *mut i32, WNOHANG as i32);
            
            poll_count += 1;
            
            if result == 0 {
                // Child not ready yet
                if !got_zero {
                    print("✓ Got 0 from WNOHANG (child still running)\n");
                    got_zero = true;
                }
                
                // Don't poll too aggressively
                sleep_ticks(10);
            } else if result == child_pid {
                // Child exited
                print("✓ Child exited after ");
                print(itoa(poll_count, &mut buf));
                print(" polls, status: ");
                print(itoa(status & 0xFF, &mut buf));
                print("\n");
                
                // Verify status
                if (status & 0xFF) != 42 {
                    print("ERROR: Expected status 42, got ");
                    print(itoa(status & 0xFF, &mut buf));
                    print("\n");
                    sys_exit(1);
                }
                
                // Verify we got at least one 0 result
                if !got_zero {
                    print("ERROR: Never got 0 from WNOHANG\n");
                    sys_exit(1);
                }
                
                break;
            } else {
                // Error
                print("ERROR: waitpid returned ");
                print(itoa(result as u32, &mut buf));
                print("\n");
                sys_exit(1);
            }
        }
        
        print("✓ wait_nohang_polling test passed!\n");
        sys_exit(0);
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    unsafe {
        print("wait_nohang_polling test panicked!\n");
        sys_exit(1);
    }
}