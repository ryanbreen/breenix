//! rm - remove files
//!
//! Usage: rm FILE
//!
//! Removes the specified file (not directories - use rmdir).

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libbreenix::argv;
use libbreenix::errno::Errno;
use libbreenix::fs::unlink;
use libbreenix::io::{println, stderr};
use libbreenix::process::exit;

fn print_error(path: &[u8], e: Errno) {
    let _ = stderr().write_str("rm: cannot remove '");
    let _ = stderr().write(path);
    let _ = stderr().write_str("': ");
    let _ = stderr().write_str(match e {
        Errno::ENOENT => "No such file or directory",
        Errno::EACCES => "Permission denied",
        Errno::EISDIR => "Is a directory",
        Errno::EBUSY => "Device or resource busy",
        _ => "Error",
    });
    let _ = stderr().write(b"\n");
}

fn print_usage() {
    let _ = stderr().write_str("Usage: rm FILE\n");
}

#[no_mangle]
pub extern "C" fn main(argc: usize, argv_ptr: *const *const u8) -> i32 {
    let args = unsafe { argv::Args::new(argc, argv_ptr) };

    if args.argc < 2 {
        print_usage();
        return 1;
    }

    let file_arg = match args.argv(1) {
        Some(arg) => arg,
        None => {
            print_usage();
            return 1;
        }
    };

    // Build the path
    const PATH_BUF_LEN: usize = 256;
    let mut path_buf = [0u8; PATH_BUF_LEN];
    let path_len;

    if file_arg.starts_with(b"/") {
        if file_arg.len() >= PATH_BUF_LEN {
            let _ = stderr().write_str("rm: path too long\n");
            return 1;
        }
        path_buf[..file_arg.len()].copy_from_slice(file_arg);
        path_len = file_arg.len();
    } else {
        if file_arg.len() + 1 >= PATH_BUF_LEN {
            let _ = stderr().write_str("rm: path too long\n");
            return 1;
        }
        path_buf[0] = b'/';
        path_buf[1..=file_arg.len()].copy_from_slice(file_arg);
        path_len = file_arg.len() + 1;
    }
    path_buf[path_len] = 0;

    let path_str = match core::str::from_utf8(&path_buf[..=path_len]) {
        Ok(s) => s,
        Err(_) => {
            let _ = stderr().write_str("rm: invalid path encoding\n");
            return 1;
        }
    };

    match unlink(path_str) {
        Ok(()) => {
            println("rm: file removed");
            0
        }
        Err(e) => {
            print_error(file_arg, e);
            1
        }
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    let _ = stderr().write_str("rm: panic!\n");
    exit(2);
}
