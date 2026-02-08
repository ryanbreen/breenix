//! Simple exit test (std version)
//!
//! Just exits with code 42, no other output.
//! This is used to test exec from ext2 without breakpoint handling.

fn main() {
    // Just exit with code 42 - no printing
    std::process::exit(42);
}
