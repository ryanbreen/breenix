#![no_std]
#![no_main]

include!("libbreenix.rs");

use core::panic::PanicInfo;

// Simple print function
unsafe fn print(s: &str) {
    sys_write(1, s.as_bytes());
}

// Print a number
unsafe fn print_num(n: i32) {
    if n == 0 {
        print("0");
        return;
    }
    
    let mut num = n;
    let mut digits = [0u8; 10];
    let mut i = 0;
    
    while num > 0 {
        digits[i] = b'0' + (num % 10) as u8;
        num /= 10;
        i += 1;
    }
    
    // Print digits in reverse order
    while i > 0 {
        i -= 1;
        let s = core::str::from_utf8(&digits[i..i+1]).unwrap_or("?");
        print(s);
    }
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    unsafe {
        print("Fork spin stress test: Creating 50 children that busy-loop\n");
        
        const NUM_CHILDREN: i32 = 50;
        let mut child_count = 0;
        
        // Fork 50 children
        for i in 0..NUM_CHILDREN {
            let pid = sys_fork() as i64;
            if pid == 0 {
                // Child process - busy loop for a while
                print("Child ");
                print_num(i);
                print(" spinning...\n");
                
                // Busy loop - do some computation
                let mut sum: u64 = 0;
                for j in 0..100000 {
                    sum = sum.wrapping_add(j);
                    // Add some more operations to ensure multiple instructions
                    if sum > 1000000 {
                        sum = sum.wrapping_sub(500000);
                    }
                }
                
                print("Child ");
                print_num(i);
                print(" done spinning, sum = ");
                print_num((sum % 1000000) as i32);
                print("\n");
                
                sys_exit(i);
            } else if pid > 0 {
                // Parent
                child_count += 1;
                print("Parent: Created child ");
                print_num(child_count);
                print(" with PID ");
                print_num(pid as i32);
                print("\n");
            } else {
                print("Fork failed!\n");
                break;
            }
        }
        
        // Parent waits for all children
        print("\nParent: Waiting for all ");
        print_num(child_count);
        print(" children to complete...\n");
        
        let mut completed = 0;
        while completed < child_count {
            let mut status: u32 = 0;
            let pid = sys_waitpid(-1, &mut status, 0);
            if pid > 0 {
                completed += 1;
                print("Parent: Child PID ");
                print_num(pid as i32);
                print(" exited with status ");
                print_num(status as i32);
                print(" (");
                print_num(completed);
                print("/");
                print_num(child_count);
                print(" done)\n");
            } else {
                print("Wait failed!\n");
                break;
            }
        }
        
        if completed == child_count {
            print("\nSUCCESS: All ");
            print_num(child_count);
            print(" children completed!\n");
        } else {
            print("\nFAILURE: Only ");
            print_num(completed);
            print(" of ");
            print_num(child_count);
            print(" children completed\n");
        }
        
        sys_exit(0)
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    unsafe {
        print("Panic in fork_spin_stress test!\n");
        sys_exit(1)
    }
}