//! head - output the first part of files
//!
//! Usage: head [-n NUM] [FILE...]
//!
//! Print the first 10 lines of each FILE to standard output.
//! With more than one FILE, precede each with a header giving the file name.
//!
//! Options:
//!   -n NUM   print the first NUM lines instead of the first 10

use std::env;
use std::fs::File;
use std::io::{self, BufRead, BufReader, Write};
use std::process;

const DEFAULT_LINES: usize = 10;

/// Read and output up to `max_lines` lines from a reader
fn head_from_reader<R: BufRead>(reader: R, max_lines: usize) -> io::Result<()> {
    if max_lines == 0 {
        return Ok(());
    }

    let stdout = io::stdout();
    let mut out = stdout.lock();
    let mut lines_output = 0;

    for line_result in reader.lines() {
        let line = line_result?;
        writeln!(out, "{}", line)?;
        lines_output += 1;
        if lines_output >= max_lines {
            break;
        }
    }

    Ok(())
}

/// Read and output up to `max_lines` lines from stdin
fn head_stdin(max_lines: usize) -> io::Result<()> {
    let stdin = io::stdin();
    let reader = stdin.lock();
    head_from_reader(reader, max_lines)
}

/// Read and output up to `max_lines` lines from a file
fn head_file(path: &str, max_lines: usize) -> io::Result<()> {
    let file = File::open(path)?;
    let reader = BufReader::new(file);
    head_from_reader(reader, max_lines)
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
                        eprintln!("head: invalid number of lines");
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
                        eprintln!("head: invalid number of lines");
                        process::exit(1);
                    }
                }
            }
        }
    }

    // If no files, read from stdin
    if file_start_idx >= args.len() {
        if let Err(_) = head_stdin(max_lines) {
            eprintln!("head: error reading stdin");
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

        if let Err(e) = head_file(path, max_lines) {
            eprintln!("head: {}: {}", path, e);
            exit_code = 1;
        }
    }

    process::exit(exit_code);
}
