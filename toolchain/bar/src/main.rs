#![forbid(unsafe_code)]

fn main() -> std::process::ExitCode {
    bar::run(std::env::args_os().skip(1))
}
