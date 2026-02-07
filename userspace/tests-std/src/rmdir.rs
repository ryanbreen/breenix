//! rmdir - remove empty directories
//!
//! Usage: rmdir DIRECTORY
//!
//! Removes the specified directory (must be empty).

use std::env;
use std::fs;
use std::process;

fn main() {
    let args: Vec<String> = env::args().collect();

    if args.len() < 2 {
        eprintln!("Usage: rmdir DIRECTORY");
        process::exit(1);
    }

    let path = &args[1];

    match fs::remove_dir(path) {
        Ok(()) => {
            println!("rmdir: directory removed");
        }
        Err(e) => {
            eprintln!("rmdir: failed to remove '{}': {}", path, e);
            process::exit(1);
        }
    }
}
