//! mv - move (rename) files
//!
//! Usage: mv SOURCE DEST
//!
//! Renames SOURCE to DEST.

use std::env;
use std::fs;
use std::process;

fn main() {
    let args: Vec<String> = env::args().collect();

    if args.len() < 3 {
        eprintln!("Usage: mv SOURCE DEST");
        process::exit(1);
    }

    let src = &args[1];
    let dst = &args[2];

    match fs::rename(src, dst) {
        Ok(()) => {
            println!("mv: file moved");
        }
        Err(e) => {
            eprintln!("mv: cannot move '{}' to '{}': {}", src, dst, e);
            process::exit(1);
        }
    }
}
