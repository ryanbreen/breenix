//! mv - move (rename) files
//!
//! Usage: mv SOURCE DEST
//!
//! Renames SOURCE to DEST.

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libbreenix::argv::get_args;
use libbreenix::errno::Errno;
use libbreenix::fs::rename;
use libbreenix::io::{println, stderr};
use libbreenix::process::exit;

fn print_error(src: &[u8], dst: &[u8], e: Errno) {
    let _ = stderr().write_str("mv: cannot move '");
    let _ = stderr().write(src);
    let _ = stderr().write_str("' to '");
    let _ = stderr().write(dst);
    let _ = stderr().write_str("': ");
    let _ = stderr().write_str(match e {
        Errno::ENOENT => "No such file or directory",
        Errno::EACCES => "Permission denied",
        Errno::EISDIR => "Is a directory",
        Errno::ENOTDIR => "Not a directory",
        Errno::EEXIST => "File exists",
        Errno::ENOTEMPTY => "Directory not empty",
        _ => "Error",
    });
    let _ = stderr().write(b"\n");
}

fn print_usage() {
    let _ = stderr().write_str("Usage: mv SOURCE DEST\n");
}

/// Build a null-terminated path string from argv bytes
fn build_path(arg: &[u8], buf: &mut [u8; 256]) -> Option<usize> {
    if arg.starts_with(b"/") {
        if arg.len() >= 256 {
            return None;
        }
        buf[..arg.len()].copy_from_slice(arg);
        buf[arg.len()] = 0;
        Some(arg.len())
    } else {
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
pub extern "C" fn _start() -> ! {
    let args = unsafe { get_args() };

    if args.argc < 3 {
        print_usage();
        exit(1);
    }

    let src_arg = match args.argv(1) {
        Some(arg) => arg,
        None => {
            print_usage();
            exit(1);
        }
    };

    let dst_arg = match args.argv(2) {
        Some(arg) => arg,
        None => {
            print_usage();
            exit(1);
        }
    };

    let mut src_buf = [0u8; 256];
    let mut dst_buf = [0u8; 256];

    let src_len = match build_path(src_arg, &mut src_buf) {
        Some(len) => len,
        None => {
            let _ = stderr().write_str("mv: source path too long\n");
            exit(1);
        }
    };

    let dst_len = match build_path(dst_arg, &mut dst_buf) {
        Some(len) => len,
        None => {
            let _ = stderr().write_str("mv: destination path too long\n");
            exit(1);
        }
    };

    let src = match core::str::from_utf8(&src_buf[..=src_len]) {
        Ok(s) => s,
        Err(_) => {
            let _ = stderr().write_str("mv: invalid source path encoding\n");
            exit(1);
        }
    };

    let dst = match core::str::from_utf8(&dst_buf[..=dst_len]) {
        Ok(s) => s,
        Err(_) => {
            let _ = stderr().write_str("mv: invalid destination path encoding\n");
            exit(1);
        }
    };

    match rename(src, dst) {
        Ok(()) => {
            println("mv: file moved");
            exit(0)
        }
        Err(e) => {
            print_error(src_arg, dst_arg, e);
            exit(1);
        }
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    let _ = stderr().write_str("mv: panic!\n");
    exit(2);
}
