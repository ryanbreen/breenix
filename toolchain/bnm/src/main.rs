#![forbid(unsafe_code)]

fn main() -> std::process::ExitCode {
    bnm::run(std::env::args_os().skip(1))
}
