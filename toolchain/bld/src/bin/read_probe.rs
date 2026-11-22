#![forbid(unsafe_code)]

fn main() -> std::process::ExitCode {
    bld::run_read_probe(std::env::args_os().skip(1))
}
