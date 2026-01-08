#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libbreenix::argv;
use libbreenix::io::{close, fcntl_getfd, fd_flags, pipe2, println, read, status_flags, write};
use libbreenix::process::{execv, exit, fork, waitpid, wexitstatus, wifexited};

const EXEC_CHECK_ARG: &[u8] = b"--exec-check";

#[no_mangle]
pub extern "C" fn _start() -> ! {
    println("=== Close-on-Exec Test ===");

    let args = unsafe { argv::get_args() };
    if args.argc >= 2 {
        if let Some(arg1) = args.argv(1) {
            if bytes_eq(arg1, EXEC_CHECK_ARG) {
                exec_check(args);
            }
        }
    }

    let mut fds = [0i32; 2];
    if pipe2(&mut fds, status_flags::O_CLOEXEC) < 0 {
        println("pipe2 failed");
        exit(1);
    }

    let read_fd = fds[0] as u64;
    let write_fd = fds[1] as u64;

    let flags = fcntl_getfd(read_fd);
    if flags != fd_flags::FD_CLOEXEC as i64 {
        println("FAIL: FD_CLOEXEC not set on pipe read fd");
        exit(1);
    }

    let data = b"test";
    if write(write_fd, data) < 0 {
        println("write failed");
        exit(1);
    }
    close(write_fd);

    let pid = unsafe { fork() };
    if pid == 0 {
        let mut fd_buf = [0u8; 21];
        let fd_str = format_u64(read_fd, &mut fd_buf[..20]);
        fd_buf[fd_str.len()] = 0;

        let program = b"cloexec_test\0";
        let arg0 = b"cloexec_test\0";
        let arg1 = b"--exec-check\0";
        let argv = [
            arg0.as_ptr(),
            arg1.as_ptr(),
            fd_buf.as_ptr(),
            core::ptr::null(),
        ];

        let _ = unsafe { execv(program, argv.as_ptr()) };
        println("exec failed");
        exit(1);
    } else if pid > 0 {
        close(read_fd);
        let mut status: i32 = 0;
        let _ = waitpid(pid as i32, &mut status, 0);
        if wifexited(status) {
            exit(wexitstatus(status));
        }
        println("child did not exit cleanly");
        exit(1);
    } else {
        println("fork failed");
        exit(1);
    }
}

fn exec_check(args: argv::Args) -> ! {
    if args.argc < 3 {
        println("FAIL: missing fd argument");
        exit(1);
    }
    let fd_bytes = match args.argv(2) {
        Some(val) => val,
        None => {
            println("FAIL: missing fd argument");
            exit(1);
        }
    };

    let fd = match parse_u64(fd_bytes) {
        Some(val) => val,
        None => {
            println("FAIL: invalid fd argument");
            exit(1);
        }
    };

    let mut buf = [0u8; 4];
    let ret = read(fd, &mut buf);
    if ret >= 0 {
        println("FAIL: read succeeded after exec");
        exit(1);
    }
    if ret != -9 {
        println("WARN: expected EBADF (-9)");
    }
    println("CLOEXEC_TEST_PASSED");
    exit(0);
}

fn bytes_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    for i in 0..a.len() {
        if a[i] != b[i] {
            return false;
        }
    }
    true
}

fn parse_u64(buf: &[u8]) -> Option<u64> {
    if buf.is_empty() {
        return None;
    }
    let mut value: u64 = 0;
    for &b in buf {
        if b == 0 {
            break;
        }
        if b < b'0' || b > b'9' {
            return None;
        }
        value = value.saturating_mul(10).saturating_add((b - b'0') as u64);
    }
    Some(value)
}

fn format_u64(mut n: u64, buf: &mut [u8]) -> &[u8] {
    if n == 0 {
        buf[0] = b'0';
        return &buf[..1];
    }

    let mut i = buf.len();
    while n > 0 && i > 0 {
        i -= 1;
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
    }

    &buf[i..]
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    exit(255);
}
