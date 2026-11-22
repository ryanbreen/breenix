#![forbid(unsafe_code)]

use std::env;
use std::io::{self, BufWriter, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use bcc::pp::{self, Options, Target};
use bcc::{codegen, sema};

enum Mode {
    Preprocess,
    Syntax,
    DumpTypes,
    Assembly,
}

fn arguments() -> Result<(PathBuf, Options, Mode), String> {
    let mut options = Options::default();
    let mut args = env::args_os().skip(1);
    let mut file = None;
    let mut preprocess = false;
    let mut syntax = false;
    let mut dump_types = false;
    let mut assembly = false;
    let mut positional = false;
    while let Some(arg) = args.next() {
        match arg.to_str().filter(|_| !positional) {
            Some("-E") => preprocess = true,
            Some("-fsyntax-only") => syntax = true,
            Some("--dump-types") => dump_types = true,
            Some("-S") => assembly = true,
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
    let mode = match (assembly, preprocess, syntax, dump_types) {
        (true, false, false, false) => Mode::Assembly,
        (true, _, _, _) => return Err("-S cannot be combined with other compilation modes".into()),
        (false, true, false, false) => Mode::Preprocess,
        (false, false, _, true) => Mode::DumpTypes,
        (false, false, true, false) => Mode::Syntax,
        (false, false, false, false) => return Err("expected a compilation mode".into()),
        _ => return Err("-E cannot be combined with syntax checking".into()),
    };
    let file = file.ok_or("expected one source file")?;
    // The installed bundle puts its compiler headers and libc headers beside
    // bin/. Keep the build host's system headers out of a cross-target build.
    if let Ok(executable) = env::current_exe() {
        if let Some(bundle) = executable.parent().and_then(|bin| bin.parent()) {
            options
                .system_dirs
                .extend([bundle.join("include-support"), bundle.join("include")]);
        }
    }
    Ok((file, options, mode))
}

fn run(file: PathBuf, options: &Options, mode: Mode) -> Result<(), String> {
    let source = pp::preprocess(&file, options).map_err(|error| error.to_string())?;
    let mut out = BufWriter::new(io::stdout().lock());
    let result = match mode {
        Mode::Preprocess => source.write(&mut out),
        Mode::Syntax | Mode::DumpTypes | Mode::Assembly => {
            let unit =
                sema::analyze(&source, &file, options.target).map_err(|error| error.to_string())?;
            match mode {
                Mode::Syntax => Ok(()),
                Mode::DumpTypes => unit.write_types(&mut out),
                Mode::Assembly => {
                    let assembly = codegen::emit_assembly(&unit, &file, options.target)
                        .map_err(|error| error.to_string())?;
                    out.write_all(assembly.as_bytes())
                }
                Mode::Preprocess => unreachable!(),
            }
        }
    };
    result
        .and_then(|()| out.flush())
        .map_err(|error| format!("bcc: error writing output: {error}"))
}

fn main() -> ExitCode {
    let (file, options, mode) = match arguments() {
        Ok(args) => args,
        Err(error) => {
            eprintln!("bcc: {error}\nusage: bcc (-E|-S|-fsyntax-only|--dump-types) [--target=x86_64|aarch64] [-I DIR] FILE");
            return ExitCode::from(2);
        }
    };
    match run(file, &options, mode) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}
