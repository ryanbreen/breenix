//! rm - remove files
//!
//! Usage: rm FILE
//!
//! Removes the specified file (not directories - use rmdir).

use std::env;
use std::fs;
use std::process;

fn main() {
    let args: Vec<String> = env::args().collect();

    if args.len() < 2 {
        eprintln!("Usage: rm FILE");
        process::exit(1);
    }

    let path = &args[1];

    match fs::remove_file(path) {
        Ok(()) => {
            println!("rm: file removed");
        }
        Err(e) => {
            eprintln!("rm: cannot remove '{}': {}", path, e);
            process::exit(1);
        }
    }
}
