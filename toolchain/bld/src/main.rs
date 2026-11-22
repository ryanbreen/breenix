#![forbid(unsafe_code)]

fn main() -> std::process::ExitCode {
    eprintln!("bld: static linking is not implemented");
    std::process::ExitCode::FAILURE
}
