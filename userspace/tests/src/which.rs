//! which - locate a command
//!
//! Usage: which COMMAND
//!
//! Search PATH (/bin, /sbin) for COMMAND and print its full path.
//! Exits 0 if found, 1 if not found.

use libbreenix::fs;

/// PATH directories to search, in order
const PATH_DIRS: &[&str] = &["/bin/", "/sbin/"];

/// Check if a file exists and is executable at the given path.
///
/// Uses libbreenix::fs::access() with X_OK to check execute permission.
fn is_executable(path: &str) -> bool {
    let c_path = format!("{}\0", path);
    fs::access(&c_path, fs::X_OK).is_ok()
}

fn main() {
    let args: Vec<String> = std::env::args().collect();

    if args.len() < 2 {
        eprintln!("which: missing command name");
        eprintln!("Usage: which COMMAND");
        std::process::exit(1);
    }

    let cmd_name = &args[1];
    if cmd_name.is_empty() {
        eprintln!("which: empty command name");
        std::process::exit(1);
    }

    // If command contains '/', it's an explicit path - check it directly
    if cmd_name.contains('/') {
        if is_executable(cmd_name) {
            println!("{}", cmd_name);
            std::process::exit(0);
        } else {
            std::process::exit(1);
        }
    }

    // Search PATH directories
    for dir in PATH_DIRS {
        let full_path = format!("{}{}", dir, cmd_name);
        if is_executable(&full_path) {
            println!("{}", full_path);
            std::process::exit(0);
        }
    }

    // Not found
    std::process::exit(1);
}
