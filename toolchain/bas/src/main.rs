#![forbid(unsafe_code)]

use std::env;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::Path;
use std::process::ExitCode;

fn assemble_file(output: &Path, source: &Path) -> Result<(), String> {
    let io_error = |path: &Path, error: io::Error| format!("{}: {error}", path.display());
    let bytes = fs::read(source).map_err(|error| io_error(source, error))?;
    let object = bas::assemble(&bytes).map_err(|error| format!("{}:{error}", source.display()))?;
    fs::write(output, object).map_err(|error| io_error(output, error))
}

fn main() -> ExitCode {
    let args: Vec<OsString> = env::args_os().skip(1).collect();
    let result = match args.as_slice() {
        [option, output, source]
            if option == "-o" && !source.as_encoded_bytes().starts_with(b"-") =>
        {
            assemble_file(Path::new(output), Path::new(source))
        }
        _ => Err("usage: bas -o OBJECT SOURCE".into()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}
