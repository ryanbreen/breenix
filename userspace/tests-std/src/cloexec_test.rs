//! Close-on-exec (FD_CLOEXEC) test program (std version)
//!
//! Tests that FD_CLOEXEC flag is properly set by pipe2(O_CLOEXEC) and
//! that marked file descriptors are closed across execve().
//!
//! Test flow:
//! 1. Create pipe with O_CLOEXEC
//! 2. Verify FD_CLOEXEC is set via fcntl
//! 3. Fork, child re-execs self with "--exec-check <fd_num>"
//! 4. In exec-check mode: attempt read from fd, expect EBADF (-9)

use std::env;

const O_CLOEXEC: i32 = 0o2000000;
const FD_CLOEXEC: i32 = 1;
const F_GETFD: i32 = 1;

extern "C" {
    fn fork() -> i32;
    fn waitpid(pid: i32, status: *mut i32, options: i32) -> i32;
    fn pipe2(pipefd: *mut i32, flags: i32) -> i32;
    fn fcntl(fd: i32, cmd: i32, ...) -> i32;
    fn read(fd: i32, buf: *mut u8, count: usize) -> isize;
    fn write(fd: i32, buf: *const u8, count: usize) -> isize;
    fn close(fd: i32) -> i32;
    fn execve(path: *const u8, argv: *const *const u8, envp: *const *const u8) -> i32;
}

/// POSIX WIFEXITED: true if child terminated normally
fn wifexited(status: i32) -> bool {
    (status & 0x7f) == 0
}

/// POSIX WEXITSTATUS: extract exit code from status
fn wexitstatus(status: i32) -> i32 {
    (status >> 8) & 0xff
}

/// Exec-check mode: verify that the given fd is closed (EBADF)
fn exec_check(fd_str: &str) -> ! {
    let fd: i32 = match fd_str.parse() {
        Ok(v) => v,
        Err(_) => {
            println!("FAIL: invalid fd argument '{}'", fd_str);
            std::process::exit(1);
        }
    };

    let mut buf = [0u8; 4];
    let ret = unsafe { read(fd, buf.as_mut_ptr(), buf.len()) };
    if ret >= 0 {
        println!("FAIL: read succeeded after exec (fd {} should be closed)", fd);
        std::process::exit(1);
    }
    if ret != -9 {
        println!("WARN: expected EBADF (-9), got {}", ret);
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
    let mut fds = [0i32; 2];
    let ret = unsafe { pipe2(fds.as_mut_ptr(), O_CLOEXEC) };
    if ret < 0 {
        println!("pipe2 failed with {}", ret);
        std::process::exit(1);
    }

    let read_fd = fds[0];
    let write_fd = fds[1];
    println!("Created pipe with O_CLOEXEC: read_fd={}, write_fd={}", read_fd, write_fd);

    // Step 2: Verify FD_CLOEXEC is set
    let flags = unsafe { fcntl(read_fd, F_GETFD) };
    if flags != FD_CLOEXEC {
        println!("FAIL: FD_CLOEXEC not set on pipe read fd (flags={})", flags);
        std::process::exit(1);
    }
    println!("PASS: FD_CLOEXEC is set on read fd");

    // Write some data so the fd is "readable" if not closed
    let data = b"test";
    let write_ret = unsafe { write(write_fd, data.as_ptr(), data.len()) };
    if write_ret < 0 {
        println!("write failed with {}", write_ret);
        std::process::exit(1);
    }
    unsafe { close(write_fd); }

    // Step 3: Fork and re-exec self with --exec-check
    let pid = unsafe { fork() };
    if pid == 0 {
        // Child: re-exec self with --exec-check <read_fd>
        let fd_string = format!("{}\0", read_fd);

        let program = b"/userspace/tests-std/cloexec_test.elf\0";
        let arg0 = b"cloexec_test\0";
        let arg1 = b"--exec-check\0";
        let argv: [*const u8; 4] = [
            arg0.as_ptr(),
            arg1.as_ptr(),
            fd_string.as_ptr(),
            std::ptr::null(),
        ];
        let envp: [*const u8; 1] = [std::ptr::null()];

        unsafe {
            execve(program.as_ptr(), argv.as_ptr(), envp.as_ptr());
        }

        // If we get here, exec failed
        println!("exec failed");
        std::process::exit(1);
    } else if pid > 0 {
        // Parent: close our copy of read_fd, then wait for child
        unsafe { close(read_fd); }

        let mut status: i32 = 0;
        unsafe { waitpid(pid, &mut status, 0); }

        if wifexited(status) {
            std::process::exit(wexitstatus(status));
        }
        println!("child did not exit cleanly");
        std::process::exit(1);
    } else {
        println!("fork failed");
        std::process::exit(1);
    }
}
