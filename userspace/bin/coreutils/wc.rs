//! wc - word, line, and byte count
//!
//! Usage: wc [OPTION]... [FILE]...
//!
//! Print newline, word, and byte counts for each FILE.
//! With no FILE, read standard input.
//!
//! Options:
//!   -c    print byte counts
//!   -l    print line counts
//!   -w    print word counts
//!
//! If no options given, print all three counts (lines words bytes).

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libbreenix::argv;
use libbreenix::errno::Errno;
use libbreenix::fs::{close, open, read, O_RDONLY};
use libbreenix::io::{stdout, stderr};
use libbreenix::process::exit;

const BUF_SIZE: usize = 4096;

struct Counts {
    lines: usize,
    words: usize,
    bytes: usize,
}

impl Counts {
    fn new() -> Self {
        Counts {
            lines: 0,
            words: 0,
            bytes: 0,
        }
    }

    fn add(&mut self, other: &Counts) {
        self.lines += other.lines;
        self.words += other.words;
        self.bytes += other.bytes;
    }
}

struct Options {
    show_lines: bool,
    show_words: bool,
    show_bytes: bool,
}

impl Options {
    fn new() -> Self {
        Options {
            show_lines: true,
            show_words: true,
            show_bytes: true,
        }
    }

    fn from_flags(lines: bool, words: bool, bytes: bool) -> Self {
        // If no flags specified, show all
        if !lines && !words && !bytes {
            Options::new()
        } else {
            Options {
                show_lines: lines,
                show_words: words,
                show_bytes: bytes,
            }
        }
    }
}

/// POSIX-compliant whitespace check: space, tab, newline, carriage return,
/// form feed (\f = 0x0c), and vertical tab (\v = 0x0b)
fn is_whitespace(c: u8) -> bool {
    c == b' ' || c == b'\t' || c == b'\n' || c == b'\r' || c == 0x0c || c == 0x0b
}

/// Count lines, words, bytes from stdin
fn wc_stdin() -> Result<Counts, Errno> {
    let mut buf = [0u8; BUF_SIZE];
    let mut counts = Counts::new();
    let mut in_word = false;

    loop {
        let n = libbreenix::io::read(0, &mut buf);
        if n < 0 {
            return Err(Errno::from_raw(-n));
        }
        if n == 0 {
            break; // EOF
        }

        counts.bytes += n as usize;

        for i in 0..n as usize {
            if buf[i] == b'\n' {
                counts.lines += 1;
            }

            if is_whitespace(buf[i]) {
                in_word = false;
            } else if !in_word {
                in_word = true;
                counts.words += 1;
            }
        }
    }

    Ok(counts)
}

/// Count lines, words, bytes from a file
fn wc_file(path: &[u8]) -> Result<Counts, Errno> {
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
    let mut counts = Counts::new();
    let mut in_word = false;

    loop {
        let n = read(fd, &mut buf)?;
        if n == 0 {
            break; // EOF
        }

        counts.bytes += n;

        for i in 0..n {
            if buf[i] == b'\n' {
                counts.lines += 1;
            }

            if is_whitespace(buf[i]) {
                in_word = false;
            } else if !in_word {
                in_word = true;
                counts.words += 1;
            }
        }
    }

    let _ = close(fd);
    Ok(counts)
}

/// Print a number right-justified in a field
fn print_num(n: usize) {
    // Convert number to digits
    if n == 0 {
        let _ = stdout().write_str("       0");
        return;
    }

    let mut buf = [0u8; 20];
    let mut i = 0;
    let mut num = n;
    while num > 0 {
        buf[i] = b'0' + (num % 10) as u8;
        num /= 10;
        i += 1;
    }

    // Right-justify in 8 characters
    let padding = if i < 8 { 8 - i } else { 0 };
    for _ in 0..padding {
        let _ = stdout().write(b" ");
    }

    // Print digits in reverse
    while i > 0 {
        i -= 1;
        let _ = stdout().write(&buf[i..i + 1]);
    }
}

fn print_counts(counts: &Counts, opts: &Options, filename: Option<&[u8]>) {
    if opts.show_lines {
        print_num(counts.lines);
    }
    if opts.show_words {
        print_num(counts.words);
    }
    if opts.show_bytes {
        print_num(counts.bytes);
    }

    if let Some(name) = filename {
        let _ = stdout().write(b" ");
        let _ = stdout().write(name);
    }
    let _ = stdout().write(b"\n");
}

fn print_error_bytes(path: &[u8], e: Errno) {
    let _ = stderr().write_str("wc: ");
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

    let mut show_lines = false;
    let mut show_words = false;
    let mut show_bytes = false;
    let mut file_start_idx = 1usize;

    // Parse options
    for i in 1..args.argc {
        if let Some(arg) = args.argv(i) {
            if arg.len() >= 2 && arg[0] == b'-' {
                // Option flag(s)
                for j in 1..arg.len() {
                    match arg[j] {
                        b'l' => show_lines = true,
                        b'w' => show_words = true,
                        b'c' => show_bytes = true,
                        _ => {
                            let _ = stderr().write_str("wc: invalid option -- '");
                            let _ = stderr().write(&arg[j..j + 1]);
                            let _ = stderr().write_str("'\n");
                            return 1;
                        }
                    }
                }
                file_start_idx = i + 1;
            } else {
                // Not an option, must be a filename
                break;
            }
        }
    }

    let opts = Options::from_flags(show_lines, show_words, show_bytes);

    // If no files, read from stdin
    if file_start_idx >= args.argc {
        match wc_stdin() {
            Ok(counts) => print_counts(&counts, &opts, None),
            Err(_e) => {
                let _ = stderr().write_str("wc: error reading stdin\n");
                return 1;
            }
        }
        return 0;
    }

    // Process files
    let mut exit_code = 0;
    let mut total = Counts::new();
    let file_count = args.argc - file_start_idx;

    for i in file_start_idx..args.argc {
        if let Some(path) = args.argv(i) {
            match wc_file(path) {
                Ok(counts) => {
                    print_counts(&counts, &opts, Some(path));
                    total.add(&counts);
                }
                Err(e) => {
                    print_error_bytes(path, e);
                    exit_code = 1;
                }
            }
        }
    }

    // Print total if multiple files
    if file_count > 1 {
        print_counts(&total, &opts, Some(b"total"));
    }

    exit_code
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    let _ = stderr().write_str("wc: panic!\n");
    exit(2);
}
