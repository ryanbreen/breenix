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

use std::env;
use std::fs::File;
use std::io::{self, Read, Write};
use std::process;

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

/// Count lines, words, bytes from a reader
fn wc_reader<R: Read>(reader: &mut R) -> io::Result<Counts> {
    let mut buf = [0u8; 4096];
    let mut counts = Counts::new();
    let mut in_word = false;

    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
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

    Ok(counts)
}

/// Count lines, words, bytes from stdin
fn wc_stdin() -> io::Result<Counts> {
    let stdin = io::stdin();
    let mut reader = stdin.lock();
    wc_reader(&mut reader)
}

/// Count lines, words, bytes from a file
fn wc_file(path: &str) -> io::Result<Counts> {
    let mut file = File::open(path)?;
    wc_reader(&mut file)
}

fn print_counts(counts: &Counts, opts: &Options, filename: Option<&str>) {
    let stdout = io::stdout();
    let mut out = stdout.lock();

    if opts.show_lines {
        let _ = write!(out, "{:>8}", counts.lines);
    }
    if opts.show_words {
        let _ = write!(out, "{:>8}", counts.words);
    }
    if opts.show_bytes {
        let _ = write!(out, "{:>8}", counts.bytes);
    }

    if let Some(name) = filename {
        let _ = write!(out, " {}", name);
    }
    let _ = writeln!(out);
}

fn main() {
    let args: Vec<String> = env::args().collect();

    let mut show_lines = false;
    let mut show_words = false;
    let mut show_bytes = false;
    let mut file_start_idx = 1usize;

    // Parse options
    for i in 1..args.len() {
        let arg = &args[i];
        if arg.len() >= 2 && arg.starts_with('-') {
            // Option flag(s)
            for c in arg[1..].chars() {
                match c {
                    'l' => show_lines = true,
                    'w' => show_words = true,
                    'c' => show_bytes = true,
                    _ => {
                        eprintln!("wc: invalid option -- '{}'", c);
                        process::exit(1);
                    }
                }
            }
            file_start_idx = i + 1;
        } else {
            // Not an option, must be a filename
            break;
        }
    }

    let opts = Options::from_flags(show_lines, show_words, show_bytes);

    // If no files, read from stdin
    if file_start_idx >= args.len() {
        match wc_stdin() {
            Ok(counts) => print_counts(&counts, &opts, None),
            Err(_) => {
                eprintln!("wc: error reading stdin");
                process::exit(1);
            }
        }
        process::exit(0);
    }

    // Process files
    let mut exit_code = 0;
    let mut total = Counts::new();
    let file_count = args.len() - file_start_idx;

    for i in file_start_idx..args.len() {
        let path = &args[i];
        match wc_file(path) {
            Ok(counts) => {
                print_counts(&counts, &opts, Some(path));
                total.add(&counts);
            }
            Err(e) => {
                eprintln!("wc: {}: {}", path, e);
                exit_code = 1;
            }
        }
    }

    // Print total if multiple files
    if file_count > 1 {
        print_counts(&total, &opts, Some("total"));
    }

    process::exit(exit_code);
}
