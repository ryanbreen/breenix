#![forbid(unsafe_code)]

use bmake::{graph::Graph, reader, run};
use std::{
    env, fs,
    io::{self, Write},
    path::PathBuf,
    process::ExitCode,
};

fn build_makefile() -> Result<(), String> {
    let mut arguments = env::args_os().skip(1);
    let (Some(path), None) = (arguments.next(), arguments.next()) else {
        return Err("usage: bmake MAKEFILE".into());
    };
    let path = PathBuf::from(path);
    let source = fs::read(&path).map_err(|error| format!("{}: {error}", path.display()))?;
    let makefile = reader::parse(&path, &source).map_err(|error| error.to_string())?;
    let graph = Graph::from_makefile(&makefile).map_err(|error| error.to_string())?;
    run::build(&graph)
}

fn main() -> ExitCode {
    match build_makefile() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let _ = writeln!(io::stderr(), "{error}");
            ExitCode::from(2)
        }
    }
}
