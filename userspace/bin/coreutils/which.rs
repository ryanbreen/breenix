//! which - locate a command
//!
//! Usage: which COMMAND
//!
//! Search PATH (/bin, /sbin) for COMMAND and print its full path.
//! Exits 0 if found, 1 if not found.

#![no_std]
#![no_main]

use core::arch::naked_asm;
use core::panic::PanicInfo;
use libbreenix::argv;
use libbreenix::fs::{access, X_OK};
use libbreenix::io::{print, println, stderr};
use libbreenix::process::exit;

/// PATH directories to search, in order
const PATH_DIRS: [&[u8]; 2] = [b"/bin/", b"/sbin/"];

/// Check if a file exists and is executable at the given path
fn is_executable(path: &[u8]) -> bool {
    // Create null-terminated path string
    let mut buf = [0u8; 256];
    if path.len() >= 256 {
        return false;
    }
    buf[..path.len()].copy_from_slice(path);
    buf[path.len()] = 0;

    // Try to convert to &str for access()
    let path_str = match core::str::from_utf8(&buf[..path.len() + 1]) {
        Ok(s) => s,
        Err(_) => return false,
    };

    // Check if executable
    access(path_str, X_OK).is_ok()
}

/// Build path: dir + name into buf, returns length
fn build_path(dir: &[u8], name: &[u8], buf: &mut [u8; 256]) -> Option<usize> {
    let total = dir.len() + name.len();
    if total >= 256 {
        return None;
    }
    buf[..dir.len()].copy_from_slice(dir);
    buf[dir.len()..total].copy_from_slice(name);
    buf[total] = 0;
    Some(total)
}

/// Naked entry point that captures RSP before any prologue modifies it.
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
extern "C" fn rust_main(stack_ptr: *const u64) -> ! {
    let args = unsafe { argv::get_args_from_stack(stack_ptr) };

    if args.argc < 2 {
        let _ = stderr().write(b"which: missing command name\n");
        let _ = stderr().write(b"Usage: which COMMAND\n");
        exit(1);
    }

    // Get command name (first argument after program name)
    let cmd_name = match args.argv(1) {
        Some(name) if !name.is_empty() => name,
        _ => {
            let _ = stderr().write(b"which: empty command name\n");
            exit(1);
        }
    };

    // If command contains '/', it's an explicit path - check it directly
    if cmd_name.iter().any(|&c| c == b'/') {
        if is_executable(cmd_name) {
            print_bytes(cmd_name);
            println("");
            exit(0);
        } else {
            exit(1);
        }
    }

    // Search PATH directories
    let mut path_buf = [0u8; 256];
    for dir in &PATH_DIRS {
        if let Some(len) = build_path(dir, cmd_name, &mut path_buf) {
            if is_executable(&path_buf[..len]) {
                print_bytes(&path_buf[..len]);
                println("");
                exit(0);
            }
        }
    }

    // Not found
    exit(1);
}

/// Print bytes as string
fn print_bytes(bytes: &[u8]) {
    if let Ok(s) = core::str::from_utf8(bytes) {
        print(s);
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    let _ = stderr().write(b"which: panic!\n");
    exit(2);
}
