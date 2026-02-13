//! bls - list directory contents
//!
//! Usage: bls [DIRECTORY]
//!
//! Lists entries in DIRECTORY (default: current directory).
//! Shows file type indicators: / for directories, @ for symlinks.
//! Entries are sorted alphabetically by name.

use std::env;
use std::fs;
use std::process;

/// A collected directory entry with its display string
struct Entry {
    name: String,
    display: String,
}

fn ls_directory(path: &str) -> Result<(), String> {
    let dir = fs::read_dir(path).map_err(|e| {
        format!("bls: cannot access '{}': {}", path, e)
    })?;

    let mut entries: Vec<Entry> = Vec::new();

    for entry in dir {
        let entry = entry.map_err(|e| {
            format!("bls: error reading entry: {}", e)
        })?;

        let name = entry.file_name();
        let name_str = name.to_string_lossy().into_owned();

        // Skip . and ..
        if name_str == "." || name_str == ".." {
            continue;
        }

        let file_type = entry.file_type().map_err(|e| {
            format!("bls: error reading file type: {}", e)
        })?;

        let display = if file_type.is_dir() {
            format!("{}/", name_str)
        } else if file_type.is_symlink() {
            format!("{}@", name_str)
        } else {
            name_str.clone()
        };

        entries.push(Entry { name: name_str, display });
    }

    // Sort alphabetically by name
    entries.sort_by(|a, b| a.name.cmp(&b.name));

    for entry in &entries {
        println!("{}", entry.display);
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
