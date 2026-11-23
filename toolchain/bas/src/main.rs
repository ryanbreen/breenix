#![forbid(unsafe_code)]

use std::env;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::Path;
use std::process::ExitCode;

use bas::Target;

mod default_target;

const USAGE: &str = "usage: bas [--target x86_64|aarch64] -o OBJECT SOURCE";

fn assemble_file(target: Target, args: &[OsString]) -> Result<(), String> {
    let [option, output, source] = args else {
        return Err(USAGE.into());
    };
    if option != "-o" || source.as_encoded_bytes().starts_with(b"-") {
        return Err(USAGE.into());
    }
    let output = Path::new(output);
    let source = Path::new(source);
    let io_error = |path: &Path, error: io::Error| format!("{}: {error}", path.display());
    let bytes = fs::read(source).map_err(|error| io_error(source, error))?;
    let object = bas::assemble_for(target, &bytes)
        .map_err(|error| format!("{}:{error}", source.display()))?;
    fs::write(output, object).map_err(|error| io_error(output, error))
}

fn main() -> ExitCode {
    let args: Vec<OsString> = env::args_os().skip(1).collect();
    let result = match args.as_slice() {
        [flag, name, rest @ ..] if flag == "--target" => match name.to_str() {
            Some("aarch64") => assemble_file(Target::Aarch64, rest),
            Some("x86_64") => assemble_file(Target::X86_64, rest),
            _ => Err(USAGE.into()),
        },
        _ => {
            let target = match default_target::default_target(env::consts::OS, env::consts::ARCH) {
                "aarch64" => Target::Aarch64,
                _ => Target::X86_64,
            };
            assemble_file(target, &args)
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}
