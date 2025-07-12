//! Fork test program

#![no_std]
#![no_main]

mod libbreenix;
use libbreenix::{sys_write, sys_exit, sys_getpid, sys_fork, sys_exec};

/// Buffer for number to string conversion
static mut BUFFER: [u8; 32] = [0; 32];

/// Convert number to string and print it
unsafe fn print_number(prefix: &str, num: u64) {
    // Print prefix
    let _ = sys_write(1, prefix.as_bytes());
    
    // Convert number to string
    let mut n = num;
    let mut i = 0;
    
    if n == 0 {
        BUFFER[0] = b'0';
        i = 1;
    } else {
        while n > 0 {
            BUFFER[i] = b'0' + (n % 10) as u8;
            n /= 10;
            i += 1;
        }
        // Reverse the digits
        for j in 0..i/2 {
            let tmp = BUFFER[j];
            BUFFER[j] = BUFFER[i - j - 1];
            BUFFER[i - j - 1] = tmp;
        }
    }
    
    let _ = sys_write(1, &BUFFER[..i]);
    let _ = sys_write(1, b"\n");
}

/// Main entry point
#[no_mangle]
pub extern "C" fn _start() -> ! {
    unsafe {
        // Get current PID before fork
        let pid_before = sys_getpid();
        print_number("Before fork - PID: ", pid_before);
        
        // Call fork
        let _ = sys_write(1, b"Calling fork()...\n");
        let fork_result = sys_fork();
        
        // Debug: Print the actual fork result value
        print_number("Fork returned value: ", fork_result);
        
        // Get PID after fork
        let pid_after = sys_getpid();
        
        // Add explicit debugging for fork result values
        if fork_result == 0 {
            let _ = sys_write(1, b"DETECTED: fork_result == 0, this is the CHILD process\n");
            // Child process
            let _ = sys_write(1, b"CHILD: Fork returned 0\n");
            print_number("CHILD: PID after fork: ", pid_after);
            
            // Exec hello_time.elf in the child process
            let _ = sys_write(1, b"CHILD: Executing hello_time.elf...\n");
            let exec_result = sys_exec("/userspace/tests/hello_time.elf", "");
            
            // If exec succeeds, this code should never run
            let _ = sys_write(1, b"CHILD: ERROR - exec returned, this shouldn't happen!\n");
            print_number("CHILD: exec returned: ", exec_result);
            
            let _ = sys_write(1, b"CHILD: Exiting with code 42\n");
            sys_exit(42);
        } else {
            // Parent process  
            let _ = sys_write(1, b"DETECTED: fork_result != 0, this is the PARENT process\n");
            let _ = sys_write(1, b"PARENT: Fork returned child PID: ");
            print_number("", fork_result);
            print_number("PARENT: PID after fork: ", pid_after);
            
            // Do some parent work
            for i in 0..3 {
                print_number("PARENT: iteration ", i);
                // Small delay
                for _ in 0..1000000 {
                    core::ptr::read_volatile(&0u8);
                }
            }
            
            let _ = sys_write(1, b"PARENT: Exiting with code 0\n");
            sys_exit(0);
        }
    }
}

/// Panic handler
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    unsafe {
        let _ = sys_write(1, b"PANIC in fork test!\n");
        sys_exit(255);
    }
}