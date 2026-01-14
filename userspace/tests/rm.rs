//! rm - remove files
//!
//! Usage: rm FILE
//!
//! Removes the specified file (not directories - use rmdir).
//! Currently uses hardcoded path until argv support is added.

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libbreenix::errno::Errno;
use libbreenix::fs::unlink;
use libbreenix::io::{println, stderr};
use libbreenix::process::exit;

fn print_error(path: &str, e: Errno) {
    let _ = stderr().write_str("rm: cannot remove '");
    let _ = stderr().write_str(path);
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

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // Demo: remove /testfile.txt (would need to be created first)
    let file_path = "/testfile.txt\0";

    match unlink(file_path) {
        Ok(()) => {
            println("rm: file removed");
            exit(0)
        }
        Err(e) => {
            print_error(file_path, e);
            exit(1);
        }
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    let _ = stderr().write_str("rm: panic!\n");
    exit(2);
}
