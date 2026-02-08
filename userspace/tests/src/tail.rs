//! tail - output the last part of files
//!
//! Usage: tail [-n NUM] [FILE...]
//!
//! Print the last 10 lines of each FILE to standard output.
//! With more than one FILE, precede each with a header giving the file name.
//!
//! Options:
//!   -n NUM   print the last NUM lines instead of the last 10

use std::collections::VecDeque;
use std::env;
use std::fs::File;
use std::io::{self, BufRead, BufReader, Write};
use std::process;

const DEFAULT_LINES: usize = 10;

/// Read all lines from a reader and output the last `max_lines` lines
fn tail_from_reader<R: BufRead>(reader: R, max_lines: usize) -> io::Result<()> {
    let mut ring: VecDeque<String> = VecDeque::with_capacity(max_lines + 1);

    for line_result in reader.lines() {
        let line = line_result?;
        ring.push_back(line);
        if ring.len() > max_lines {
            ring.pop_front();
        }
    }

    let stdout = io::stdout();
    let mut out = stdout.lock();
    for line in &ring {
        writeln!(out, "{}", line)?;
    }

    Ok(())
}

/// Read from stdin and output last N lines
fn tail_stdin(max_lines: usize) -> io::Result<()> {
    let stdin = io::stdin();
    let reader = stdin.lock();
    tail_from_reader(reader, max_lines)
}

/// Read from a file and output last N lines
fn tail_file(path: &str, max_lines: usize) -> io::Result<()> {
    let file = File::open(path)?;
    let reader = BufReader::new(file);
    tail_from_reader(reader, max_lines)
}

fn print_header(path: &str) {
    println!("==> {} <==", path);
}

fn main() {
    let args: Vec<String> = env::args().collect();

    let mut max_lines = DEFAULT_LINES;
    let mut file_start_idx = 1usize;

    // Parse -n option
    if args.len() >= 2 {
        let arg = &args[1];
        if arg.len() >= 2 && arg.starts_with("-n") {
            if arg.len() > 2 {
                // -nNUM format
                match arg[2..].parse::<usize>() {
                    Ok(n) => {
                        max_lines = n;
                        file_start_idx = 2;
                    }
                    Err(_) => {
                        eprintln!("tail: invalid number of lines");
                        process::exit(1);
                    }
                }
            } else if args.len() >= 3 {
                // -n NUM format
                match args[2].parse::<usize>() {
                    Ok(n) => {
                        max_lines = n;
                        file_start_idx = 3;
                    }
                    Err(_) => {
                        eprintln!("tail: invalid number of lines");
                        process::exit(1);
                    }
                }
            }
        }
    }

    // If no files, read from stdin
    if file_start_idx >= args.len() {
        if let Err(_) = tail_stdin(max_lines) {
            eprintln!("tail: error reading stdin");
            process::exit(1);
        }
        process::exit(0);
    }

    // Process files
    let mut exit_code = 0;
    let file_count = args.len() - file_start_idx;
    let mut first_file = true;

    for i in file_start_idx..args.len() {
        let path = &args[i];

        // Print header if multiple files
        if file_count > 1 {
            if !first_file {
                println!();
            }
            print_header(path);
        }
        first_file = false;

        if let Err(e) = tail_file(path, max_lines) {
            eprintln!("tail: {}: {}", path, e);
            exit_code = 1;
        }
    }

    process::exit(exit_code);
}
