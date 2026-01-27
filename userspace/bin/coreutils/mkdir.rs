//! mkdir - create directories
//!
//! Usage: mkdir DIRECTORY
//!
//! Creates the specified directory.

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libbreenix::argv;
use libbreenix::errno::Errno;
use libbreenix::fs::mkdir;
use libbreenix::io::{println, stderr};
use libbreenix::process::exit;

fn print_error(path: &[u8], e: Errno) {
    let _ = stderr().write_str("mkdir: cannot create directory '");
    let _ = stderr().write(path);
    let _ = stderr().write_str("': ");
    let _ = stderr().write_str(match e {
        Errno::EEXIST => "File exists",
        Errno::ENOENT => "No such file or directory",
        Errno::EACCES => "Permission denied",
        Errno::ENOTDIR => "Not a directory",
        Errno::ENOSPC => "No space left on device",
        _ => "Error",
    });
    let _ = stderr().write(b"\n");
}

fn print_usage() {
    let _ = stderr().write_str("Usage: mkdir DIRECTORY\n");
}

#[no_mangle]
pub extern "C" fn main(argc: usize, argv_ptr: *const *const u8) -> i32 {
    let args = unsafe { argv::Args::new(argc, argv_ptr) };

    // Need at least one argument (the directory name)
    if args.argc < 2 {
        print_usage();
        return 1;
    }

    // Get the directory path from argv[1]
    let dir_arg = match args.argv(1) {
        Some(arg) => arg,
        None => {
            print_usage();
            return 1;
        }
    };

    // Build the path - if relative, prepend /
    const PATH_BUF_LEN: usize = 256;
    let mut path_buf = [0u8; PATH_BUF_LEN];
    let path_len;

    if dir_arg.starts_with(b"/") {
        // Absolute path - copy as-is
        if dir_arg.len() >= PATH_BUF_LEN {
            let _ = stderr().write_str("mkdir: path too long\n");
            return 1;
        }
        path_buf[..dir_arg.len()].copy_from_slice(dir_arg);
        path_len = dir_arg.len();
    } else {
        // Relative path - prepend /
        if dir_arg.len() + 1 >= PATH_BUF_LEN {
            let _ = stderr().write_str("mkdir: path too long\n");
            return 1;
        }
        path_buf[0] = b'/';
        path_buf[1..=dir_arg.len()].copy_from_slice(dir_arg);
        path_len = dir_arg.len() + 1;
    }
    // Null terminate
    path_buf[path_len] = 0;

    // Convert to str for mkdir (it expects &str with null terminator)
    let path_str = match core::str::from_utf8(&path_buf[..=path_len]) {
        Ok(s) => s,
        Err(_) => {
            let _ = stderr().write_str("mkdir: invalid path encoding\n");
            return 1;
        }
    };

    match mkdir(path_str, 0o755) {
        Ok(()) => {
            println("mkdir: directory created");
            0
        }
        Err(e) => {
            print_error(dir_arg, e);
            1
        }
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    let _ = stderr().write_str("mkdir: panic!\n");
    exit(2);
}
