#![forbid(unsafe_code)]

fn main() -> std::process::ExitCode {
    bld::run(std::env::args_os().skip(1))
}
