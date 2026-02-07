//! ls - list directory contents
//!
//! Usage: ls [DIRECTORY]
//!
//! Lists entries in DIRECTORY (default: current directory).
//! Shows file type indicators: / for directories, @ for symlinks.

use std::env;
use std::fs;
use std::process;

fn ls_directory(path: &str) -> Result<(), String> {
    let entries = fs::read_dir(path).map_err(|e| {
        format!("ls: cannot access '{}': {}", path, e)
    })?;

    for entry in entries {
        let entry = entry.map_err(|e| {
            format!("ls: error reading entry: {}", e)
        })?;

        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        // Skip . and ..
        if name_str == "." || name_str == ".." {
            continue;
        }

        let file_type = entry.file_type().map_err(|e| {
            format!("ls: error reading file type: {}", e)
        })?;

        // Print name with type indicator
        if file_type.is_dir() {
            println!("{}/", name_str);
        } else if file_type.is_symlink() {
            println!("{}@", name_str);
        } else {
            println!("{}", name_str);
        }
    }

    Ok(())
}

fn main() {
    let args: Vec<String> = env::args().collect();

    let path = if args.len() >= 2 {
        &args[1]
    } else {
        "."
    };

    if let Err(e) = ls_directory(path) {
        eprintln!("{}", e);
        process::exit(1);
    }
}
