//! ls - list directory contents
//!
//! Usage: ls [DIRECTORY]
//!
//! Lists entries in DIRECTORY (default: root directory).
//! Shows file type indicators: / for directories, @ for symlinks.

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libbreenix::argv;
use libbreenix::errno::Errno;
use libbreenix::fs::{close, getdents64, open, DirentIter, O_DIRECTORY, O_RDONLY, DT_DIR, DT_LNK};
use libbreenix::io::{print, println, stderr};
use libbreenix::process::exit;

const BUF_SIZE: usize = 2048;

fn ls_directory(path: &[u8]) -> Result<(), Errno> {
    // Create null-terminated path for open syscall
    let mut path_buf = [0u8; 256];
    let len = path.len().min(255);
    path_buf[..len].copy_from_slice(&path[..len]);
    path_buf[len] = 0;

    // Convert to &str for open
    let path_str = match core::str::from_utf8(&path_buf[..len+1]) {
        Ok(s) => s,
        Err(_) => return Err(Errno::EINVAL),
    };

    let fd = open(path_str, O_RDONLY | O_DIRECTORY)?;

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

fn print_error(path: &[u8], e: Errno) {
    let _ = stderr().write_str("ls: cannot access '");
    let _ = stderr().write(path);
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
pub extern "C" fn main(argc: usize, argv_ptr: *const *const u8) -> i32 {
    let args = unsafe { argv::Args::new(argc, argv_ptr) };

    // Default to current directory if no arguments
    let path: &[u8] = if args.argc >= 2 {
        args.argv(1).unwrap_or(b".")
    } else {
        b"."
    };

    match ls_directory(path) {
        Ok(()) => 0,
        Err(e) => {
            print_error(path, e);
            1
        }
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    let _ = stderr().write_str("ls: panic!\n");
    exit(2);
}
