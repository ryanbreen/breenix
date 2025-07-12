//! Fork memory independence test - verify parent and child have separate memory

#![no_std]
#![no_main]

include!("libbreenix.rs");

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
    
    let mut buf = [0u8; 10];
    let mut i = 9;
    let mut num = n;
    
    while num > 0 {
        buf[i] = b'0' + (num % 10) as u8;
        num /= 10;
        i -= 1;
    }
    
    sys_write(1, &buf[i + 1..10]);
}

// Global variable to test memory independence
static mut GLOBAL_VAR: i32 = 1;

#[no_mangle]
pub extern "C" fn _start() -> ! {
    unsafe {
        print("fork_mem_independent: Starting test\n");
        print("fork_mem_independent: Initial global var = ");
        print_num(GLOBAL_VAR);
        print("\n");
        
        // Fork a child
        let pid = sys_fork() as i64;
        
        if pid < 0 {
            print("fork_mem_independent: Fork failed!\n");
            sys_exit(1);
        } else if pid == 0 {
            // Child process - modify the global variable
            print("fork_mem_independent: Child setting global var to 2\n");
            GLOBAL_VAR = 2;
            print("fork_mem_independent: Child global var = ");
            print_num(GLOBAL_VAR);
            print("\n");
            sys_exit(0);
        } else {
            // Parent process - wait for child then check variable
            print("fork_mem_independent: Parent waiting for child\n");
            
            let mut status: u32 = 0;
            let wait_result = wait(&mut status as *mut u32 as *mut i32);
            
            if wait_result != pid {
                print("✗ fork_mem_independent: Wait failed\n");
                sys_exit(1);
            }
            
            print("fork_mem_independent: Parent checking global var\n");
            print("fork_mem_independent: Parent global var = ");
            print_num(GLOBAL_VAR);
            print("\n");
            
            if GLOBAL_VAR == 1 {
                print("✓ fork_mem_independent: Test passed - memory is independent\n");
                sys_exit(0);
            } else {
                print("✗ fork_mem_independent: Test failed - memory was shared!\n");
                sys_exit(1);
            }
        }
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    unsafe {
        print("fork_mem_independent: Panic!\n");
        sys_exit(1);
    }
}