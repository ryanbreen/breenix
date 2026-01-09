#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libbreenix::io::println;
use libbreenix::process::{execv, exit, fork, waitpid, wexitstatus, wifexited};

// Test that fork+exec with argv works correctly.
#[no_mangle]
pub extern "C" fn _start() -> ! {
    println("=== Exec Argv Test ===");

    let pid = fork();
    if pid == 0 {
        // Child: exec argv_test with specific args.
        let program = b"argv_test\0";
        let arg0 = b"argv_test\0" as *const u8;
        let arg1 = b"hello\0" as *const u8;
        let arg2 = b"world\0" as *const u8;
        let argv: [*const u8; 4] = [arg0, arg1, arg2, core::ptr::null()];

        let _ = execv(program, argv.as_ptr());
        // If we get here, exec failed.
        println("exec failed");
        exit(1);
    } else if pid > 0 {
        // Parent: wait for child.
        let mut status: i32 = 0;
        let _ = waitpid(pid as i32, &mut status, 0);

        if wifexited(status) && wexitstatus(status) == 0 {
            println("EXEC_ARGV_TEST_PASSED");
            exit(0);
        } else {
            println("EXEC_ARGV_TEST_FAILED");
            exit(1);
        }
    } else {
        println("fork failed");
        exit(1);
    }
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    exit(255);
}
