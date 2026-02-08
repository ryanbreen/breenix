//! mkdir - create directories
//!
//! Usage: mkdir DIRECTORY
//!
//! Creates the specified directory.

use std::env;
use std::fs;
use std::process;

fn main() {
    let args: Vec<String> = env::args().collect();

    if args.len() < 2 {
        eprintln!("Usage: mkdir DIRECTORY");
        process::exit(1);
    }

    let path = &args[1];

    match fs::create_dir(path) {
        Ok(()) => {
            println!("mkdir: directory created");
        }
        Err(e) => {
            eprintln!("mkdir: cannot create directory '{}': {}", path, e);
            process::exit(1);
        }
    }
}
