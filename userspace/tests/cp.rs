//! cp - copy files
//!
//! Usage: cp SOURCE DEST
//!
//! Copies SOURCE to DEST. Does not support recursive directory copy.
//! Currently uses hardcoded paths until argv support is added.

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libbreenix::errno::Errno;
use libbreenix::fs::{close, open, open_with_mode, read, write, O_CREAT, O_RDONLY, O_TRUNC, O_WRONLY};
use libbreenix::io::{println, stderr};
use libbreenix::process::exit;

const BUF_SIZE: usize = 4096;

fn copy_file(src: &str, dst: &str) -> Result<(), (Errno, bool)> {
    // Open source file
    let src_fd = open(src, O_RDONLY).map_err(|e| (e, true))?;

    // Open/create destination file
    let dst_fd = match open_with_mode(dst, O_WRONLY | O_CREAT | O_TRUNC, 0o644) {
        Ok(fd) => fd,
        Err(e) => {
            let _ = close(src_fd);
            return Err((e, false));
        }
    };

    let mut buf = [0u8; BUF_SIZE];
    let result = loop {
        match read(src_fd, &mut buf) {
            Ok(0) => break Ok(()), // EOF
            Ok(n) => {
                match write(dst_fd, &buf[..n]) {
                    Ok(written) if written == n => continue,
                    Ok(_) => break Err((Errno::EIO, false)), // Partial write
                    Err(e) => break Err((e, false)),
                }
            }
            Err(e) => break Err((e, true)),
        }
    };

    let _ = close(src_fd);
    let _ = close(dst_fd);
    result
}

fn print_error(path: &str, e: Errno) {
    let _ = stderr().write_str("cp: ");
    let _ = stderr().write_str(path);
    let _ = stderr().write_str(": ");
    let _ = stderr().write_str(match e {
        Errno::ENOENT => "No such file or directory",
        Errno::EACCES => "Permission denied",
        Errno::EISDIR => "Is a directory",
        Errno::ENOSPC => "No space left on device",
        Errno::EIO => "Input/output error",
        _ => "Error",
    });
    let _ = stderr().write(b"\n");
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // Demo: copy /hello.txt to /hello_copy.txt
    let src = "/hello.txt\0";
    let dst = "/hello_copy.txt\0";

    match copy_file(src, dst) {
        Ok(()) => {
            println("cp: file copied");
            exit(0)
        }
        Err((e, is_src)) => {
            print_error(if is_src { src } else { dst }, e);
            exit(1);
        }
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    let _ = stderr().write_str("cp: panic!\n");
    exit(2);
}
