//! mkdir - create directories
//!
//! Usage: mkdir DIRECTORY
//!
//! Creates the specified directory.
//! Currently uses hardcoded path until argv support is added.

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libbreenix::errno::Errno;
use libbreenix::fs::mkdir;
use libbreenix::io::{println, stderr};
use libbreenix::process::exit;

fn print_error(path: &str, e: Errno) {
    let _ = stderr().write_str("mkdir: cannot create directory '");
    let _ = stderr().write_str(path);
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

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // Demo: create /testdir
    let dir_path = "/testdir\0";

    match mkdir(dir_path, 0o755) {
        Ok(()) => {
            println("mkdir: directory created");
            exit(0)
        }
        Err(e) => {
            print_error(dir_path, e);
            exit(1);
        }
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    let _ = stderr().write_str("mkdir: panic!\n");
    exit(2);
}
