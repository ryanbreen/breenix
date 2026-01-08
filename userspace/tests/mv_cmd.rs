//! mv - move (rename) files
//!
//! Usage: mv SOURCE DEST
//!
//! Renames SOURCE to DEST.
//! Currently uses hardcoded paths until argv support is added.

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libbreenix::errno::Errno;
use libbreenix::fs::rename;
use libbreenix::io::{println, stderr};
use libbreenix::process::exit;

fn print_error(src: &str, dst: &str, e: Errno) {
    let _ = stderr().write_str("mv: cannot move '");
    let _ = stderr().write_str(src);
    let _ = stderr().write_str("' to '");
    let _ = stderr().write_str(dst);
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

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // Demo: rename /oldname.txt to /newname.txt (would need to exist first)
    let src = "/oldname.txt\0";
    let dst = "/newname.txt\0";

    match rename(src, dst) {
        Ok(()) => {
            println("mv: file moved");
            exit(0)
        }
        Err(e) => {
            print_error(src, dst, e);
            exit(1);
        }
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    let _ = stderr().write_str("mv: panic!\n");
    exit(2);
}
