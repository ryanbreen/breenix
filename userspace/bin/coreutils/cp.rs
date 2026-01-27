//! cp - copy files
//!
//! Usage: cp SOURCE DEST
//!
//! Copies SOURCE to DEST. Does not support recursive directory copy.

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libbreenix::argv;
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

fn print_error(path: &[u8], e: Errno) {
    let _ = stderr().write_str("cp: ");
    let _ = stderr().write(path);
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

fn print_usage() {
    let _ = stderr().write_str("Usage: cp SOURCE DEST\n");
}

/// Build a null-terminated path string from argv bytes
/// Returns the path length (excluding null terminator) or None if too long
fn build_path(arg: &[u8], buf: &mut [u8; 256]) -> Option<usize> {
    if arg.starts_with(b"/") {
        // Absolute path
        if arg.len() >= 256 {
            return None;
        }
        buf[..arg.len()].copy_from_slice(arg);
        buf[arg.len()] = 0;
        Some(arg.len())
    } else {
        // Relative path - prepend /
        if arg.len() + 1 >= 256 {
            return None;
        }
        buf[0] = b'/';
        buf[1..=arg.len()].copy_from_slice(arg);
        buf[arg.len() + 1] = 0;
        Some(arg.len() + 1)
    }
}

#[no_mangle]
pub extern "C" fn main(argc: usize, argv_ptr: *const *const u8) -> i32 {
    let args = unsafe { argv::Args::new(argc, argv_ptr) };

    if args.argc < 3 {
        print_usage();
        return 1;
    }

    let src_arg = match args.argv(1) {
        Some(arg) => arg,
        None => {
            print_usage();
            return 1;
        }
    };

    let dst_arg = match args.argv(2) {
        Some(arg) => arg,
        None => {
            print_usage();
            return 1;
        }
    };

    let mut src_buf = [0u8; 256];
    let mut dst_buf = [0u8; 256];

    let src_len = match build_path(src_arg, &mut src_buf) {
        Some(len) => len,
        None => {
            let _ = stderr().write_str("cp: source path too long\n");
            return 1;
        }
    };

    let dst_len = match build_path(dst_arg, &mut dst_buf) {
        Some(len) => len,
        None => {
            let _ = stderr().write_str("cp: destination path too long\n");
            return 1;
        }
    };

    let src = match core::str::from_utf8(&src_buf[..=src_len]) {
        Ok(s) => s,
        Err(_) => {
            let _ = stderr().write_str("cp: invalid source path encoding\n");
            return 1;
        }
    };

    let dst = match core::str::from_utf8(&dst_buf[..=dst_len]) {
        Ok(s) => s,
        Err(_) => {
            let _ = stderr().write_str("cp: invalid destination path encoding\n");
            return 1;
        }
    };

    match copy_file(src, dst) {
        Ok(()) => {
            println("cp: file copied");
            0
        }
        Err((e, is_src)) => {
            print_error(if is_src { src_arg } else { dst_arg }, e);
            1
        }
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    let _ = stderr().write_str("cp: panic!\n");
    exit(2);
}
