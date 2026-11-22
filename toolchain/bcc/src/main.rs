#![forbid(unsafe_code)]

use std::env;
use std::io::{self, BufWriter, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use bcc::pp::{self, Options, Target};

fn arguments() -> Result<(PathBuf, Options), String> {
    let mut options = Options::default();
    let mut args = env::args_os().skip(1);
    let mut file = None;
    let mut preprocess = false;
    let mut positional = false;
    while let Some(arg) = args.next() {
        match arg.to_str().filter(|_| !positional) {
            Some("-E") => preprocess = true,
            Some("-I") => {
                let dir = args.next().ok_or("-I requires a directory")?;
                options.include_dirs.push(dir.into());
            }
            Some(arg) if arg.starts_with("-I") => options.include_dirs.push(arg[2..].into()),
            Some("--target=x86_64") => options.target = Target::X86_64,
            Some("--target=aarch64") => options.target = Target::Aarch64,
            Some("--") => positional = true,
            _ if !positional && arg.as_encoded_bytes().starts_with(b"-") => {
                return Err(format!("unsupported option {}", arg.to_string_lossy()));
            }
            _ if file.is_some() => return Err("expected one source file".into()),
            _ => file = Some(PathBuf::from(arg)),
        }
    }
    let file = file
        .filter(|_| preprocess)
        .ok_or("expected -E and one source file")?;
    // The installed bundle puts its compiler headers and libc headers beside
    // bin/. Keep the build host's system headers out of a cross-target build.
    if let Ok(executable) = env::current_exe() {
        if let Some(bundle) = executable.parent().and_then(|bin| bin.parent()) {
            options
                .system_dirs
                .extend([bundle.join("include-support"), bundle.join("include")]);
        }
    }
    Ok((file, options))
}

fn run(file: PathBuf, options: &Options) -> Result<(), String> {
    let source = pp::preprocess(&file, options).map_err(|error| error.to_string())?;
    let mut out = BufWriter::new(io::stdout().lock());
    source
        .write(&mut out)
        .and_then(|()| out.flush())
        .map_err(|error| format!("bcc: error writing preprocessed output: {error}"))
}

fn main() -> ExitCode {
    let (file, options) = match arguments() {
        Ok(args) => args,
        Err(error) => {
            eprintln!("bcc: {error}\nusage: bcc -E [--target=x86_64|aarch64] [-I DIR] FILE");
            return ExitCode::from(2);
        }
    };
    match run(file, &options) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}
