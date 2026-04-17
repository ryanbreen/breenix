//! Minimal raw-write post-exec diagnostic for ARM64.

use libbreenix::{io, process, Fd};

fn main() {
    let _ = io::write(Fd::STDOUT, b"[hello_raw] start\n");
    process::exit(42);
}

