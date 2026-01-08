//! rmdir - remove empty directories
//!
//! Usage: rmdir DIRECTORY
//!
//! Removes the specified directory (must be empty).
//! Currently uses hardcoded path until argv support is added.

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libbreenix::errno::Errno;
use libbreenix::fs::rmdir;
use libbreenix::io::{println, stderr};
use libbreenix::process::exit;

fn print_error(path: &str, e: Errno) {
    let _ = stderr().write_str("rmdir: failed to remove '");
    let _ = stderr().write_str(path);
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

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // Demo: remove /testdir (would need to be created and empty first)
    let dir_path = "/testdir\0";

    match rmdir(dir_path) {
        Ok(()) => {
            println("rmdir: directory removed");
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
    let _ = stderr().write_str("rmdir: panic!\n");
    exit(2);
}
