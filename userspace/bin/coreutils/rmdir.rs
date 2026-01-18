//! rmdir - remove empty directories
//!
//! Usage: rmdir DIRECTORY
//!
//! Removes the specified directory (must be empty).

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libbreenix::argv::get_args;
use libbreenix::errno::Errno;
use libbreenix::fs::rmdir;
use libbreenix::io::{println, stderr};
use libbreenix::process::exit;

fn print_error(path: &[u8], e: Errno) {
    let _ = stderr().write_str("rmdir: failed to remove '");
    let _ = stderr().write(path);
    let _ = stderr().write_str("': ");
    let _ = stderr().write_str(match e {
        Errno::ENOENT => "No such file or directory",
        Errno::ENOTDIR => "Not a directory",
        Errno::ENOTEMPTY => "Directory not empty",
        Errno::EACCES => "Permission denied",
        Errno::EBUSY => "Device or resource busy",
        _ => "Error",
    });
    let _ = stderr().write(b"\n");
}

fn print_usage() {
    let _ = stderr().write_str("Usage: rmdir DIRECTORY\n");
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    let args = unsafe { get_args() };

    if args.argc < 2 {
        print_usage();
        exit(1);
    }

    let dir_arg = match args.argv(1) {
        Some(arg) => arg,
        None => {
            print_usage();
            exit(1);
        }
    };

    // Build the path
    const PATH_BUF_LEN: usize = 256;
    let mut path_buf = [0u8; PATH_BUF_LEN];
    let path_len;

    if dir_arg.starts_with(b"/") {
        if dir_arg.len() >= PATH_BUF_LEN {
            let _ = stderr().write_str("rmdir: path too long\n");
            exit(1);
        }
        path_buf[..dir_arg.len()].copy_from_slice(dir_arg);
        path_len = dir_arg.len();
    } else {
        if dir_arg.len() + 1 >= PATH_BUF_LEN {
            let _ = stderr().write_str("rmdir: path too long\n");
            exit(1);
        }
        path_buf[0] = b'/';
        path_buf[1..=dir_arg.len()].copy_from_slice(dir_arg);
        path_len = dir_arg.len() + 1;
    }
    path_buf[path_len] = 0;

    let path_str = match core::str::from_utf8(&path_buf[..=path_len]) {
        Ok(s) => s,
        Err(_) => {
            let _ = stderr().write_str("rmdir: invalid path encoding\n");
            exit(1);
        }
    };

    match rmdir(path_str) {
        Ok(()) => {
            println("rmdir: directory removed");
            exit(0)
        }
        Err(e) => {
            print_error(dir_arg, e);
            exit(1);
        }
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    let _ = stderr().write_str("rmdir: panic!\n");
    exit(2);
}
