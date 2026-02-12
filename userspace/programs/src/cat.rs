//! cat - concatenate and print files
//!
//! Usage: cat [FILE...]
//!
//! Reads FILE(s) and prints their contents to stdout.
//! If no FILE is specified, reads from stdin.

use std::env;
use std::fs;
use std::io::{self, Read, Write};
use std::process;

fn cat_stdin() -> io::Result<()> {
    let mut buf = [0u8; 4096];
    let stdin = io::stdin();
    let mut handle = stdin.lock();
    let mut stdout = io::stdout().lock();
    loop {
        let n = handle.read(&mut buf)?;
        if n == 0 {
            break;
        }
        stdout.write_all(&buf[..n])?;
    }
    Ok(())
}

fn cat_file(path: &str) -> io::Result<()> {
    let contents = fs::read(path)?;
    let mut stdout = io::stdout().lock();
    stdout.write_all(&contents)?;
    Ok(())
}

fn main() {
    let args: Vec<String> = env::args().collect();

    // If no arguments (besides program name), read from stdin
    if args.len() < 2 {
        if let Err(e) = cat_stdin() {
            eprintln!("cat: error reading stdin: {}", e);
            process::exit(1);
        }
        return;
    }

    // Process each file argument
    let mut exit_code = 0;
    for path in &args[1..] {
        if let Err(e) = cat_file(path) {
            eprintln!("cat: {}: {}", path, e);
            exit_code = 1;
        }
    }

    process::exit(exit_code);
}
