//! Simple exit test (std version)
//!
//! Just exits with code 0, no other output.
//! This exists so a fork+exec test target can exit successfully without
//! depending on BusyBox/ext2 coreutils such as /sbin/true being present.
//! See simple_exit.rs for the sibling this mirrors.

fn main() {
    // Just exit with code 0 - no printing
    std::process::exit(0);
}
