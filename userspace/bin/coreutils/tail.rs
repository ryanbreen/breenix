//! tail - output the last part of files
//!
//! Usage: tail [-n NUM] [FILE...]
//!
//! Print the last 10 lines of each FILE to standard output.
//! With more than one FILE, precede each with a header giving the file name.
//!
//! Options:
//!   -n NUM   print the last NUM lines instead of the last 10
//!
//! Note: This simple implementation buffers the entire file in memory,
//! so it may not work well for very large files.

#![no_std]
#![no_main]

use core::arch::naked_asm;
use core::panic::PanicInfo;
use libbreenix::argv;
use libbreenix::errno::Errno;
use libbreenix::fs::{close, open, read, O_RDONLY};
use libbreenix::io::{stdout, stderr};
use libbreenix::process::exit;

const DEFAULT_LINES: usize = 10;
// NOTE: User stack is 64KB. Keep buffer + line tracker under ~20KB to avoid overflow.
const MAX_BUF_SIZE: usize = 16384; // 16KB max file size for tail
const MAX_LINES: usize = 256; // Max lines to track (256 * 8 = 2KB)

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

/// Ring buffer to track line start positions
struct LineTracker {
    /// Start positions of lines (circular buffer)
    starts: [usize; MAX_LINES],
    /// Index of oldest line in buffer
    head: usize,
    /// Number of lines stored
    count: usize,
}

impl LineTracker {
    fn new() -> Self {
        LineTracker {
            starts: [0; MAX_LINES],
            head: 0,
            count: 0,
        }
    }

    fn push(&mut self, pos: usize) {
        let idx = (self.head + self.count) % MAX_LINES;
        if self.count < MAX_LINES {
            self.starts[idx] = pos;
            self.count += 1;
        } else {
            // Buffer full, overwrite oldest
            self.starts[self.head] = pos;
            self.head = (self.head + 1) % MAX_LINES;
        }
    }

    /// Get the start position of the Nth line from the end
    fn get_start(&self, from_end: usize) -> Option<usize> {
        if from_end >= self.count {
            // Want more lines than we have, return start
            Some(self.starts[self.head])
        } else {
            let idx = (self.head + self.count - from_end) % MAX_LINES;
            Some(self.starts[idx])
        }
    }
}

/// Read stdin into buffer and output last N lines
fn tail_stdin(max_lines: usize) -> Result<(), Errno> {
    let mut buf = [0u8; MAX_BUF_SIZE];
    let mut total_len = 0;
    let mut tracker = LineTracker::new();

    // First line starts at 0
    tracker.push(0);

    // Read entire input into buffer
    loop {
        if total_len >= MAX_BUF_SIZE {
            let _ = stderr().write_str("tail: input too large\n");
            return Err(Errno::ENOMEM);
        }

        let n = libbreenix::io::read(0, &mut buf[total_len..]);
        if n < 0 {
            return Err(Errno::from_raw(-n));
        }
        if n == 0 {
            break; // EOF
        }

        // Track line starts
        for i in total_len..(total_len + n as usize) {
            if buf[i] == b'\n' && i + 1 < MAX_BUF_SIZE {
                tracker.push(i + 1);
            }
        }

        total_len += n as usize;
    }

    // If input ends with newline, we pushed a phantom "line start" at total_len.
    // Adjust count to reflect actual number of lines.
    if tracker.count > 1 {
        let last_idx = (tracker.head + tracker.count - 1) % MAX_LINES;
        if tracker.starts[last_idx] >= total_len {
            tracker.count -= 1;
        }
    }

    // Output last N lines
    if let Some(start) = tracker.get_start(max_lines) {
        let _ = stdout().write(&buf[start..total_len]);
    }

    Ok(())
}

/// Read file into buffer and output last N lines
fn tail_file(path: &[u8], max_lines: usize) -> Result<(), Errno> {
    let mut path_buf = [0u8; 256];
    let len = path.len().min(255);
    path_buf[..len].copy_from_slice(&path[..len]);
    path_buf[len] = 0;

    let path_str = match core::str::from_utf8(&path_buf[..len + 1]) {
        Ok(s) => s,
        Err(_) => return Err(Errno::EINVAL),
    };

    let fd = open(path_str, O_RDONLY)?;

    let mut buf = [0u8; MAX_BUF_SIZE];
    let mut total_len = 0;
    let mut tracker = LineTracker::new();

    // First line starts at 0
    tracker.push(0);

    // Read entire file into buffer
    loop {
        if total_len >= MAX_BUF_SIZE {
            let _ = close(fd);
            let _ = stderr().write_str("tail: file too large\n");
            return Err(Errno::ENOMEM);
        }

        let n = read(fd, &mut buf[total_len..])?;
        if n == 0 {
            break; // EOF
        }

        // Track line starts
        for i in total_len..(total_len + n) {
            if buf[i] == b'\n' && i + 1 < MAX_BUF_SIZE {
                tracker.push(i + 1);
            }
        }

        total_len += n;
    }

    let _ = close(fd);

    // If file ends with newline, we pushed a phantom "line start" at total_len.
    // Adjust count to reflect actual number of lines.
    if tracker.count > 1 {
        let last_idx = (tracker.head + tracker.count - 1) % MAX_LINES;
        if tracker.starts[last_idx] >= total_len {
            tracker.count -= 1;
        }
    }

    // Output last N lines
    if let Some(start) = tracker.get_start(max_lines) {
        let _ = stdout().write(&buf[start..total_len]);
    }

    Ok(())
}

fn print_header(path: &[u8]) {
    let _ = stdout().write_str("==> ");
    let _ = stdout().write(path);
    let _ = stdout().write_str(" <==\n");
}

fn print_error_bytes(path: &[u8], e: Errno) {
    let _ = stderr().write_str("tail: ");
    let _ = stderr().write(path);
    let _ = stderr().write_str(": ");
    let _ = stderr().write_str(match e {
        Errno::ENOENT => "No such file or directory",
        Errno::EACCES => "Permission denied",
        Errno::EISDIR => "Is a directory",
        Errno::EINVAL => "Invalid argument",
        Errno::ENOMEM => "File too large",
        _ => "Error",
    });
    let _ = stderr().write(b"\n");
}

#[unsafe(naked)]
#[no_mangle]
pub extern "C" fn _start() -> ! {
    naked_asm!(
        "mov rdi, rsp",
        "and rsp, -16",
        "call {main}",
        "ud2",
        main = sym rust_main,
    )
}

extern "C" fn rust_main(stack_ptr: *const u64) -> ! {
    let args = unsafe { argv::get_args_from_stack(stack_ptr) };

    let mut max_lines = DEFAULT_LINES;
    let mut file_start_idx = 1usize;

    // Parse -n option
    if args.argc >= 2 {
        if let Some(arg) = args.argv(1) {
            if arg.len() >= 2 && arg[0] == b'-' && arg[1] == b'n' {
                if arg.len() > 2 {
                    // -nNUM format
                    if let Some(n) = parse_num(&arg[2..]) {
                        max_lines = n;
                        file_start_idx = 2;
                    } else {
                        let _ = stderr().write_str("tail: invalid number of lines\n");
                        exit(1);
                    }
                } else if args.argc >= 3 {
                    // -n NUM format
                    if let Some(num_arg) = args.argv(2) {
                        if let Some(n) = parse_num(num_arg) {
                            max_lines = n;
                            file_start_idx = 3;
                        } else {
                            let _ = stderr().write_str("tail: invalid number of lines\n");
                            exit(1);
                        }
                    }
                }
            }
        }
    }

    // If no files, read from stdin
    if file_start_idx >= args.argc {
        if let Err(_e) = tail_stdin(max_lines) {
            exit(1);
        }
        exit(0);
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

            if let Err(e) = tail_file(path, max_lines) {
                print_error_bytes(path, e);
                exit_code = 1;
            }
        }
    }

    exit(exit_code);
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    let _ = stderr().write_str("tail: panic!\n");
    exit(2);
}
