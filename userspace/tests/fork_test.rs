//! Fork test program
//!
//! Tests fork() and exec() syscalls.

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libbreenix::io;
use libbreenix::process;
use libbreenix::types::fd;

/// Buffer for number to string conversion
static mut BUFFER: [u8; 32] = [0; 32];

/// Convert number to string and print it
unsafe fn print_number(prefix: &str, num: u64) {
    // Print prefix
    io::print(prefix);

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
        for j in 0..i / 2 {
            let tmp = BUFFER[j];
            BUFFER[j] = BUFFER[i - j - 1];
            BUFFER[i - j - 1] = tmp;
        }
    }

    io::write(fd::STDOUT, &BUFFER[..i]);
    io::print("\n");
}

/// Main entry point
#[no_mangle]
pub extern "C" fn _start() -> ! {
    unsafe {
        // Get current PID before fork
        let pid_before = process::getpid();
        print_number("Before fork - PID: ", pid_before);

        // Call fork
        io::print("Calling fork()...\n");
        let fork_result = process::fork();

        // Debug: Print the actual fork result value
        print_number("Fork returned value: ", fork_result as u64);

        // Get PID after fork
        let pid_after = process::getpid();

        // Add explicit debugging for fork result values
        if fork_result == 0 {
            io::print("DETECTED: fork_result == 0, this is the CHILD process\n");
            // Child process
            io::print("CHILD: Fork returned 0\n");
            print_number("CHILD: PID after fork: ", pid_after);

            // Exec hello_time.elf in the child process
            io::print("CHILD: Executing hello_time.elf...\n");
            let exec_result = process::exec(b"/userspace/tests/hello_time.elf\0");

            // If exec succeeds, this code should never run
            io::print("CHILD: ERROR - exec returned, this shouldn't happen!\n");
            print_number("CHILD: exec returned: ", exec_result as u64);

            io::print("CHILD: Exiting with code 42\n");
            process::exit(42);
        } else {
            // Parent process
            io::print("DETECTED: fork_result != 0, this is the PARENT process\n");
            io::print("PARENT: Fork returned child PID: ");
            print_number("", fork_result as u64);
            print_number("PARENT: PID after fork: ", pid_after);

            // Do some parent work
            for i in 0..3u64 {
                print_number("PARENT: iteration ", i);
                // Small delay
                for _ in 0..1000000 {
                    core::ptr::read_volatile(&0u8);
                }
            }

            io::print("PARENT: Exiting with code 0\n");
            process::exit(0);
        }
    }
}

/// Panic handler
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    io::print("PANIC in fork test!\n");
    process::exit(255);
}
