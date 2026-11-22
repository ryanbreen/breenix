#![forbid(unsafe_code)]

use bmake::{graph::Graph, reader};
use std::{env, fs, path::PathBuf, process::ExitCode};

fn check_makefile() -> Result<(), String> {
    let mut arguments = env::args_os().skip(1);
    let (Some(path), None) = (arguments.next(), arguments.next()) else {
        return Err("usage: bmake MAKEFILE (read and check dependencies)".into());
    };
    let path = PathBuf::from(path);
    let source = fs::read(&path).map_err(|error| format!("{}: {error}", path.display()))?;
    let makefile = reader::parse(&path, &source).map_err(|error| error.to_string())?;
    Graph::from_makefile(&makefile).map_err(|error| error.to_string())?;
    Ok(())
}

fn main() -> ExitCode {
    match check_makefile() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::from(2)
        }
    }
}
