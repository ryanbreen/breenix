//! cp - copy files
//!
//! Usage: cp SOURCE DEST
//!
//! Copies SOURCE to DEST. Does not support recursive directory copy.

use std::env;
use std::fs;
use std::process;

fn print_usage() {
    eprintln!("Usage: cp SOURCE DEST");
}

fn main() {
    let args: Vec<String> = env::args().collect();

    if args.len() < 3 {
        print_usage();
        process::exit(1);
    }

    let src = &args[1];
    let dst = &args[2];

    match fs::copy(src, dst) {
        Ok(_) => {
            println!("cp: file copied");
        }
        Err(e) => {
            eprintln!("cp: {}: {}", src, e);
            process::exit(1);
        }
    }
}
