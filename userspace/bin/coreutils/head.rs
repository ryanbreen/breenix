//! head - output the first part of files
//!
//! Usage: head [-n NUM] [FILE...]
//!
//! Print the first 10 lines of each FILE to standard output.
//! With more than one FILE, precede each with a header giving the file name.
//!
//! Options:
//!   -n NUM   print the first NUM lines instead of the first 10

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libbreenix::argv;
use libbreenix::errno::Errno;
use libbreenix::fs::{close, open, read, O_RDONLY};
use libbreenix::io::{stdout, stderr};
use libbreenix::process::exit;

const DEFAULT_LINES: usize = 10;
const BUF_SIZE: usize = 4096;

/// Parse a number from bytes. Returns None for empty or invalid input.
fn parse_num(s: &[u8]) -> Option<usize> {
    if s.is_empty() {
        return None;
    }
    let mut n: usize = 0;
    for &c in s {
        if c < b'0' || c > b'9' {
            return None;
        }
        n = n.checked_mul(10)?.checked_add((c - b'0') as usize)?;
    }
    Some(n)
}

/// Read and output up to `max_lines` lines from stdin
fn head_stdin(max_lines: usize) -> Result<(), Errno> {
    // Early return for -n0: output nothing
    if max_lines == 0 {
        return Ok(());
    }

    let mut buf = [0u8; BUF_SIZE];
    let mut lines_output = 0;

    'outer: loop {
        let n = libbreenix::io::read(0, &mut buf);
        if n < 0 {
            return Err(Errno::from_raw(-n));
        }
        if n == 0 {
            break; // EOF
        }

        // Output bytes, counting newlines
        for i in 0..n as usize {
            let _ = stdout().write(&buf[i..i + 1]);
            if buf[i] == b'\n' {
                lines_output += 1;
                if lines_output >= max_lines {
                    break 'outer;
                }
            }
        }
    }
    Ok(())
}

/// Read and output up to `max_lines` lines from a file
fn head_file(path: &[u8], max_lines: usize) -> Result<(), Errno> {
    // Early return for -n0: output nothing
    if max_lines == 0 {
        return Ok(());
    }

    let mut path_buf = [0u8; 256];
    let len = path.len().min(255);
    path_buf[..len].copy_from_slice(&path[..len]);
    path_buf[len] = 0;

    let path_str = match core::str::from_utf8(&path_buf[..len + 1]) {
        Ok(s) => s,
        Err(_) => return Err(Errno::EINVAL),
    };

    let fd = open(path_str, O_RDONLY)?;

    let mut buf = [0u8; BUF_SIZE];
    let mut lines_output = 0;

    'outer: loop {
        let n = read(fd, &mut buf)?;
        if n == 0 {
            break; // EOF
        }

        for i in 0..n {
            let _ = stdout().write(&buf[i..i + 1]);
            if buf[i] == b'\n' {
                lines_output += 1;
                if lines_output >= max_lines {
                    break 'outer;
                }
            }
        }
    }

    let _ = close(fd);
    Ok(())
}

fn print_header(path: &[u8]) {
    let _ = stdout().write_str("==> ");
    let _ = stdout().write(path);
    let _ = stdout().write_str(" <==\n");
}

fn print_error_bytes(path: &[u8], e: Errno) {
    let _ = stderr().write_str("head: ");
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
pub extern "C" fn main(argc: usize, argv_ptr: *const *const u8) -> i32 {
    let args = unsafe { argv::Args::new(argc, argv_ptr) };

    let mut max_lines = DEFAULT_LINES;
    let mut file_start_idx = 1usize;

    // Parse -n option
    if args.argc >= 2 {
        if let Some(arg) = args.argv(1) {
            if arg.len() >= 2 && arg[0] == b'-' && arg[1] == b'n' {
                // -nNUM or -n NUM
                if arg.len() > 2 {
                    // -nNUM format
                    if let Some(n) = parse_num(&arg[2..]) {
                        max_lines = n;
                        file_start_idx = 2;
                    } else {
                        let _ = stderr().write_str("head: invalid number of lines\n");
                        return 1;
                    }
                } else if args.argc >= 3 {
                    // -n NUM format
                    if let Some(num_arg) = args.argv(2) {
                        if let Some(n) = parse_num(num_arg) {
                            max_lines = n;
                            file_start_idx = 3;
                        } else {
                            let _ = stderr().write_str("head: invalid number of lines\n");
                            return 1;
                        }
                    }
                }
            }
        }
    }

    // If no files, read from stdin
    if file_start_idx >= args.argc {
        if let Err(_e) = head_stdin(max_lines) {
            let _ = stderr().write_str("head: error reading stdin\n");
            return 1;
        }
        return 0;
    }

    // Process files
    let mut exit_code = 0;
    let file_count = args.argc - file_start_idx;
    let mut first_file = true;

    for i in file_start_idx..args.argc {
        if let Some(path) = args.argv(i) {
            // Print header if multiple files
            if file_count > 1 {
                if !first_file {
                    let _ = stdout().write(b"\n");
                }
                print_header(path);
            }
            first_file = false;

            if let Err(e) = head_file(path, max_lines) {
                print_error_bytes(path, e);
                exit_code = 1;
            }
        }
    }

    exit_code
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    let _ = stderr().write_str("head: panic!\n");
    exit(2);
}
