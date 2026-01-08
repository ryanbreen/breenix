//! cat - concatenate and print files
//!
//! Usage: cat [FILE...]
//!
//! Reads FILE(s) and prints their contents to stdout.
//! If no FILE is specified, reads from stdin (not yet implemented).
//! Supports argv for command-line argument passing.

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libbreenix::argv;
use libbreenix::errno::Errno;
use libbreenix::fs::{close, open, read, O_RDONLY};
use libbreenix::io::{stdout, stderr};
use libbreenix::process::exit;

const BUF_SIZE: usize = 4096;

fn cat_file(path: &[u8]) -> Result<(), Errno> {
    // Create null-terminated path for open syscall
    let mut path_buf = [0u8; 256];
    let len = path.len().min(255);
    path_buf[..len].copy_from_slice(&path[..len]);
    path_buf[len] = 0;

    // Convert to &str for open (which expects &str)
    let path_str = match core::str::from_utf8(&path_buf[..len+1]) {
        Ok(s) => s,
        Err(_) => return Err(Errno::EINVAL),
    };

    let fd = open(path_str, O_RDONLY)?;

    let mut buf = [0u8; BUF_SIZE];
    loop {
        let n = read(fd, &mut buf)?;
        if n == 0 {
            break; // EOF
        }
        // Write to stdout
        let _ = stdout().write(&buf[..n]);
    }

    let _ = close(fd);
    Ok(())
}

fn print_error_bytes(path: &[u8], e: Errno) {
    let _ = stderr().write_str("cat: ");
    let _ = stderr().write(path);
    let _ = stderr().write_str(": ");
    let _ = stderr().write_str(match e {
        Errno::ENOENT => "No such file or directory",
        Errno::EACCES => "Permission denied",
        Errno::EISDIR => "Is a directory",
        Errno::EINVAL => "Invalid argument",
        _ => "Error",
    });
    let _ = stderr().write(b"\n");
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // Get command-line arguments from the stack
    let args = unsafe { argv::get_args() };

    // If no arguments (besides program name), print usage and exit with error
    if args.argc < 2 {
        let _ = stderr().write_str("cat: missing file operand\n");
        let _ = stderr().write_str("Usage: cat FILE...\n");
        exit(1);
    }

    // Process each file argument
    let mut exit_code = 0;
    for i in 1..args.argc {
        if let Some(path) = args.argv(i) {
            if let Err(e) = cat_file(path) {
                print_error_bytes(path, e);
                exit_code = 1;
            }
        }
    }

    exit(exit_code);
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    let _ = stderr().write_str("cat: panic!\n");
    exit(2);
}
