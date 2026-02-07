//! false - return unsuccessful exit status
//!
//! Usage: false
//!
//! Exit with a status code indicating failure (1).
//! This command does nothing and always fails.

fn main() {
    std::process::exit(1);
}
