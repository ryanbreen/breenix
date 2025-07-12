//! Test waitpid() with specific child PIDs
//! 
//! Fork two children; _exit(7) and _exit(9).
//! Parent calls waitpid(child1, &st, 0) and then waitpid(child2, &st2, 0).
//! Validate correct pid and status each time.

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
        print("waitpid_specific test starting\n");
        
        let mut buf = [0u8; 32];
        
        // Fork first child
        let child1_pid = sys_fork() as i64;
        if child1_pid == 0 {
            // First child - exit with status 7
            print("Child 1 exiting with code 7\n");
            sys_exit(7);
        } else if child1_pid < 0 {
            print("Fork 1 failed!\n");
            sys_exit(1);
        }
        
        print("Forked child 1 with PID ");
        print(itoa(child1_pid as u32, &mut buf));
        print("\n");
        
        // Fork second child
        let child2_pid = sys_fork() as i64;
        if child2_pid == 0 {
            // Second child - exit with status 9
            print("Child 2 exiting with code 9\n");
            sys_exit(9);
        } else if child2_pid < 0 {
            print("Fork 2 failed!\n");
            sys_exit(1);
        }
        
        print("Forked child 2 with PID ");
        print(itoa(child2_pid as u32, &mut buf));
        print("\n");
        
        // Wait for child 1 specifically
        print("Waiting for child 1...\n");
        let mut status1: u32 = 0;
        let collected_pid1 = waitpid(child1_pid, &mut status1 as *mut u32 as *mut i32, 0);
        
        if collected_pid1 != child1_pid {
            print("ERROR: Expected to collect child ");
            print(itoa(child1_pid as u32, &mut buf));
            print(" but got ");
            print(itoa(collected_pid1 as u32, &mut buf));
            print("\n");
            sys_exit(1);
        }
        
        if (status1 & 0xFF) != 7 {
            print("ERROR: Expected status 7 for child 1, got ");
            print(itoa(status1 & 0xFF, &mut buf));
            print("\n");
            sys_exit(1);
        }
        
        print("✓ Collected child 1 with correct status\n");
        
        // Wait for child 2 specifically
        print("Waiting for child 2...\n");
        let mut status2: u32 = 0;
        let collected_pid2 = waitpid(child2_pid, &mut status2 as *mut u32 as *mut i32, 0);
        
        if collected_pid2 != child2_pid {
            print("ERROR: Expected to collect child ");
            print(itoa(child2_pid as u32, &mut buf));
            print(" but got ");
            print(itoa(collected_pid2 as u32, &mut buf));
            print("\n");
            sys_exit(1);
        }
        
        if (status2 & 0xFF) != 9 {
            print("ERROR: Expected status 9 for child 2, got ");
            print(itoa(status2 & 0xFF, &mut buf));
            print("\n");
            sys_exit(1);
        }
        
        print("✓ Collected child 2 with correct status\n");
        print("✓ waitpid_specific test passed!\n");
        sys_exit(0);
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    unsafe {
        print("waitpid_specific test panicked!\n");
        sys_exit(1);
    }
}