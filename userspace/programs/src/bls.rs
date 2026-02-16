//! bls - list directory contents
//!
//! Usage: bls [-l] [DIRECTORY]
//!
//! Lists entries in DIRECTORY (default: current directory).
//! Shows file type indicators: / for directories, @ for symlinks.
//! Entries are sorted alphabetically by name.
//!
//! Options:
//!   -l    Long format: show file size and type

use std::env;
use std::fs;
use std::process;

/// A collected directory entry
struct Entry {
    name: String,
    display: String,
    size: u64,
    is_dir: bool,
}

fn format_size(size: u64) -> String {
    if size < 1024 {
        format!("{}", size)
    } else if size < 1024 * 1024 {
        format!("{}K", size / 1024)
    } else {
        format!("{}M", size / (1024 * 1024))
    }
}

fn ls_directory(path: &str, long: bool) -> Result<(), String> {
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

        let is_dir = file_type.is_dir();

        let display = if is_dir {
            format!("{}/", name_str)
        } else if file_type.is_symlink() {
            format!("{}@", name_str)
        } else {
            name_str.clone()
        };

        let size = if long {
            entry.metadata().map(|m| m.len()).unwrap_or(0)
        } else {
            0
        };

        entries.push(Entry { name: name_str, display, size, is_dir });
    }

    // Sort alphabetically by name
    entries.sort_by(|a, b| a.name.cmp(&b.name));

    if long {
        // Find max size width for alignment
        let max_size = entries.iter().map(|e| format_size(e.size).len()).max().unwrap_or(0);

        for entry in &entries {
            let type_char = if entry.is_dir { 'd' } else { '-' };
            let size_str = format_size(entry.size);
            println!("{} {:>width$} {}", type_char, size_str, entry.display, width = max_size);
        }
    } else {
        for entry in &entries {
            println!("{}", entry.display);
        }
    }

    Ok(())
}

fn main() {
    let args: Vec<String> = env::args().collect();

    let mut long = false;
    let mut path = ".";

    for arg in args.iter().skip(1) {
        if arg == "-l" {
            long = true;
        } else {
            path = arg;
        }
    }

    if let Err(e) = ls_directory(path, long) {
        eprintln!("{}", e);
        process::exit(1);
    }
}
