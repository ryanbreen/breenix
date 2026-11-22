#![forbid(unsafe_code)]

use std::process::ExitCode;

fn main() -> ExitCode {
    bcc::driver::run(std::env::args_os().skip(1))
}
