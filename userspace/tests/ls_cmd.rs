//! ls - list directory contents
//!
//! Usage: ls [DIRECTORY]
//!
//! Lists entries in DIRECTORY (default: root directory).
//! Shows file type indicators: / for directories, @ for symlinks.

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libbreenix::errno::Errno;
use libbreenix::fs::{close, getdents64, open, DirentIter, O_DIRECTORY, O_RDONLY, DT_DIR, DT_LNK};
use libbreenix::io::{print, println, stderr};
use libbreenix::process::exit;

const BUF_SIZE: usize = 2048;

fn ls_directory(path: &str) -> Result<(), Errno> {
    let fd = open(path, O_RDONLY | O_DIRECTORY)?;

    let mut buf = [0u8; BUF_SIZE];

    loop {
        let n = getdents64(fd, &mut buf)?;
        if n == 0 {
            break; // End of directory
        }

        for entry in DirentIter::new(&buf, n) {
            // Skip . and ..
            let name = unsafe { entry.name() };
            if name == b"." || name == b".." {
                continue;
            }

            // Print name
            if let Ok(name_str) = core::str::from_utf8(name) {
                print(name_str);
            }

            // Add type indicator
            match entry.d_type {
                DT_DIR => print("/"),
                DT_LNK => print("@"),
                _ => {}
            }

            println("");
        }
    }

    let _ = close(fd);
    Ok(())
}

fn print_error(path: &str, e: Errno) {
    let _ = stderr().write_str("ls: cannot access '");
    let _ = stderr().write_str(path);
    let _ = stderr().write_str("': ");
    let _ = stderr().write_str(match e {
        Errno::ENOENT => "No such file or directory",
        Errno::EACCES => "Permission denied",
        Errno::ENOTDIR => "Not a directory",
        _ => "Error",
    });
    let _ = stderr().write(b"\n");
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // Default to root directory without argv
    let dir_path = "/\0";

    match ls_directory(dir_path) {
        Ok(()) => exit(0),
        Err(e) => {
            print_error(dir_path, e);
            exit(1);
        }
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    let _ = stderr().write_str("ls: panic!\n");
    exit(2);
}
