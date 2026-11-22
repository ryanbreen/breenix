#![forbid(unsafe_code)]

use std::env;
use std::fs;
use std::io::{self, BufWriter, Write};
use std::path::Path;
use std::process::ExitCode;

fn preprocess(path: &Path) -> Result<(), String> {
    let input = fs::read(path).map_err(|error| format!("{}: error: {error}", path.display()))?;
    let source = bcc::pp::scan(&input).map_err(|error| {
        format!(
            "{}:{}:{}: error: {}",
            path.display(),
            error.location.line,
            error.location.column,
            error.message
        )
    })?;
    let mut out = BufWriter::new(io::stdout().lock());
    source
        .write(&mut out)
        .and_then(|()| out.flush())
        .map_err(|error| format!("bcc: error writing preprocessed output: {error}"))
}

fn main() -> ExitCode {
    let args: Vec<_> = env::args_os().skip(1).collect();
    if let [option, file] = args.as_slice() {
        if option == "-E" && !file.as_encoded_bytes().starts_with(b"-") {
            return match preprocess(Path::new(file)) {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("{error}");
                    ExitCode::FAILURE
                }
            };
        }
    }
    eprintln!("usage: bcc -E FILE");
    ExitCode::from(2)
}
