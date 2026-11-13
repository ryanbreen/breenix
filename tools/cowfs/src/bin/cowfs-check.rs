//! cowfs-check [--full] IMAGE
//!
//! Checks a cowfs image against docs/fs/cowfs-format.md section 3 and prints one finding per
//! line: `error <ID> <detail>` or `warning <detail>`. Exit 0 clean, 1 warnings only, 4 any
//! error, 8 unreadable or no valid superblock slot. A usage error also exits 8, since no
//! image was checked.

#[path = "../check/mod.rs"]
mod check;

use std::ffi::OsString;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::exit;

const USAGE: &str = "usage: cowfs-check [--full] IMAGE";

fn main() {
    let mut full = false;
    let mut image: Option<PathBuf> = None;
    for arg in std::env::args_os().skip(1) {
        if arg == "--full" {
            full = true;
        } else if arg == "-h" || arg == "--help" {
            println!("{USAGE}");
            exit(0);
        } else if image.is_none() && !starts_with_dash(&arg) {
            image = Some(PathBuf::from(arg));
        } else {
            eprintln!("cowfs-check: unexpected argument {}\n{USAGE}", arg.to_string_lossy());
            exit(check::EXIT_UNREADABLE);
        }
    }
    let Some(image) = image else {
        eprintln!("{USAGE}");
        exit(check::EXIT_UNREADABLE);
    };
    let outcome = check::check(&image, full);
    let mut out = io::stdout().lock();
    for line in &outcome.lines {
        if writeln!(out, "{line}").is_err() {
            break;
        }
    }
    let _ = out.flush();
    if let Some(why) = outcome.fatal {
        eprintln!("cowfs-check: {why}");
    }
    exit(outcome.code);
}

fn starts_with_dash(arg: &OsString) -> bool {
    arg.to_string_lossy().starts_with('-')
}
