//! cat - concatenate and print files
//!
//! Usage: cat [FILE...]
//!
//! Reads FILE(s) and prints their contents to stdout.
//! If no FILE is specified, reads from stdin.
//! Supports argv for command-line argument passing.

#![no_std]
#![no_main]

use core::arch::naked_asm;
use core::panic::PanicInfo;
use libbreenix::argv;
use libbreenix::errno::Errno;
use libbreenix::fs::{close, open, read, O_RDONLY};
use libbreenix::io::{stdout, stderr};
use libbreenix::process::exit;

const BUF_SIZE: usize = 4096;

/// Read from stdin and write to stdout
fn cat_stdin() -> Result<(), Errno> {
    let mut buf = [0u8; BUF_SIZE];
    loop {
        let n = libbreenix::io::read(0, &mut buf); // fd 0 = stdin
        if n < 0 {
            return Err(Errno::from_raw(-n));
        }
        if n == 0 {
            break; // EOF
        }
        let _ = stdout().write(&buf[..n as usize]);
    }
    Ok(())
}

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

/// Naked entry point that captures RSP before any prologue modifies it.
/// RSP points to argc on entry per Linux x86_64 ABI.
#[unsafe(naked)]
#[no_mangle]
pub extern "C" fn _start() -> ! {
    naked_asm!(
        "mov rdi, rsp",    // Pass original RSP as first argument
        "and rsp, -16",    // Align stack to 16 bytes (ABI requirement)
        "call {main}",     // Call rust_main(stack_ptr)
        "ud2",             // Should never return
        main = sym rust_main,
    )
}

/// Real entry point called from naked _start with the original stack pointer.
/// Note: stack_ptr points to the ORIGINAL RSP (argc location), not current RSP.
extern "C" fn rust_main(stack_ptr: *const u64) -> ! {
    // Get command-line arguments from the original stack pointer
    // stack_ptr was captured BEFORE the call instruction, so it points to argc
    let args = unsafe { argv::get_args_from_stack(stack_ptr) };

    // If no arguments (besides program name), read from stdin
    if args.argc < 2 {
        if let Err(_e) = cat_stdin() {
            let _ = stderr().write_str("cat: error reading stdin\n");
            exit(1);
        }
        exit(0);
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
