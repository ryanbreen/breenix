//! Fork deep stack test - test fork with deep recursion

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

// Recursive function that forks at each level
unsafe fn recursive_fork(depth: i32, max_depth: i32) {
    // Local variable to test stack independence
    let local_var = depth;
    
    print("fork_deep_stack: Depth ");
    print_num(depth);
    print(" (local_var = ");
    print_num(local_var);
    print(")\n");
    
    if depth >= max_depth {
        print("fork_deep_stack: Reached max depth\n");
        return;
    }
    
    // Fork at this recursion level
    let pid = sys_fork() as i64;
    
    if pid < 0 {
        print("fork_deep_stack: Fork failed at depth ");
        print_num(depth);
        print("\n");
        sys_exit(1);
    } else if pid == 0 {
        // Child process - continue recursion
        print("fork_deep_stack: Child at depth ");
        print_num(depth);
        print("\n");
        
        // Verify local variable is still correct
        if local_var != depth {
            print("✗ fork_deep_stack: Stack corruption detected!\n");
            sys_exit(1);
        }
        
        recursive_fork(depth + 1, max_depth);
        
        print("fork_deep_stack: Child at depth ");
        print_num(depth);
        print(" exiting\n");
        sys_exit(depth);
    } else {
        // Parent process - wait for child
        print("fork_deep_stack: Parent at depth ");
        print_num(depth);
        print(" waiting\n");
        
        let mut status: u32 = 0;
        let wait_result = wait(&mut status as *mut u32 as *mut i32);
        
        if wait_result != pid {
            print("✗ fork_deep_stack: Wait failed at depth ");
            print_num(depth);
            print("\n");
            sys_exit(1);
        }
        
        // Verify local variable is still correct after wait
        if local_var != depth {
            print("✗ fork_deep_stack: Stack corruption after wait!\n");
            sys_exit(1);
        }
        
        print("fork_deep_stack: Parent at depth ");
        print_num(depth);
        print(" child exited with status ");
        print_num((status & 0xFF) as i32);
        print("\n");
    }
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    unsafe {
        print("fork_deep_stack: Starting deep recursion fork test\n");
        
        // Test with depth 5 (not too deep to avoid stack overflow)
        recursive_fork(0, 5);
        
        print("✓ fork_deep_stack: Test passed - no stack corruption\n");
        sys_exit(0);
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    unsafe {
        print("fork_deep_stack: Panic!\n");
        sys_exit(1);
    }
}