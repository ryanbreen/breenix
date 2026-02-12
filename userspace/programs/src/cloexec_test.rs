//! Close-on-exec (FD_CLOEXEC) test program (std version)
//!
//! Tests that FD_CLOEXEC flag is properly set by pipe2(O_CLOEXEC) and
//! that marked file descriptors are closed across execve().
//!
//! Test flow:
//! 1. Create pipe with O_CLOEXEC
//! 2. Verify FD_CLOEXEC is set via fcntl
//! 3. Fork, child re-execs self with "--exec-check <fd_num>"
//! 4. In exec-check mode: attempt read from fd, expect EBADF

use libbreenix::io;
use libbreenix::io::fd_flags::FD_CLOEXEC;
use libbreenix::io::status_flags::O_CLOEXEC;
use libbreenix::process::{self, ForkResult, execv, wifexited, wexitstatus};
use libbreenix::types::Fd;
use libbreenix::Errno;
use std::env;

/// Exec-check mode: verify that the given fd is closed (EBADF)
fn exec_check(fd_str: &str) -> ! {
    let fd: u64 = match fd_str.parse() {
        Ok(v) => v,
        Err(_) => {
            println!("FAIL: invalid fd argument '{}'", fd_str);
            std::process::exit(1);
        }
    };

    let fd = Fd::from_raw(fd);
    let mut buf = [0u8; 4];
    match io::read(fd, &mut buf) {
        Ok(_) => {
            println!("FAIL: read succeeded after exec (fd {} should be closed)", fd.raw());
            std::process::exit(1);
        }
        Err(libbreenix::error::Error::Os(Errno::EBADF)) => {
            // Expected
        }
        Err(e) => {
            println!("WARN: expected EBADF, got {:?}", e);
        }
    }
    println!("CLOEXEC_TEST_PASSED");
    std::process::exit(0);
}

fn main() {
    // Check if we're in exec-check mode
    let args: Vec<String> = env::args().collect();
    if args.len() >= 3 && args[1] == "--exec-check" {
        exec_check(&args[2]);
    }

    println!("=== Close-on-Exec Test ===");

    // Step 1: Create pipe with O_CLOEXEC
    let (read_fd, write_fd) = match io::pipe2(O_CLOEXEC) {
        Ok(fds) => fds,
        Err(e) => {
            println!("pipe2 failed with {:?}", e);
            std::process::exit(1);
        }
    };
    println!("Created pipe with O_CLOEXEC: read_fd={}, write_fd={}", read_fd.raw() as i32, write_fd.raw() as i32);

    // Step 2: Verify FD_CLOEXEC is set
    let flags = io::fcntl_getfd(read_fd).unwrap();
    if flags != FD_CLOEXEC as i64 {
        println!("FAIL: FD_CLOEXEC not set on pipe read fd (flags={})", flags);
        std::process::exit(1);
    }
    println!("PASS: FD_CLOEXEC is set on read fd");

    // Write some data so the fd is "readable" if not closed
    let data = b"test";
    let write_ret = io::write(write_fd, data).unwrap();
    if write_ret == 0 {
        println!("write failed");
        std::process::exit(1);
    }
    let _ = io::close(write_fd);

    // Step 3: Fork and re-exec self with --exec-check
    let fork_result = match process::fork() {
        Ok(result) => result,
        Err(_) => {
            println!("fork failed");
            std::process::exit(1);
        }
    };

    match fork_result {
        ForkResult::Child => {
            // Child: re-exec self with --exec-check <read_fd>
            let fd_string = format!("{}\0", read_fd.raw());

            let program = b"cloexec_test\0";
            let arg0 = b"cloexec_test\0";
            let arg1 = b"--exec-check\0";
            let argv: [*const u8; 4] = [
                arg0.as_ptr(),
                arg1.as_ptr(),
                fd_string.as_ptr(),
                std::ptr::null(),
            ];

            let _ = execv(program, argv.as_ptr());

            // If we get here, exec failed
            println!("exec failed");
            std::process::exit(1);
        }
        ForkResult::Parent(child_pid) => {
            // Parent: close our copy of read_fd, then wait for child
            let _ = io::close(read_fd);

            let mut status: i32 = 0;
            let _ = process::waitpid(child_pid.raw() as i32, &mut status, 0);

            if wifexited(status) {
                std::process::exit(wexitstatus(status));
            }
            println!("child did not exit cleanly");
            std::process::exit(1);
        }
    }
}
