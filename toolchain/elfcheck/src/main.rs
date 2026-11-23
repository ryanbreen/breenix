//! elfcheck [--machine x86_64|aarch64] [--exec] FILE...   |   elfcheck --list
//!
//! `--machine` names the processor the files must be for; it defaults to
//! x86_64. A failed rule prints `FILE: CHECK-ID: detail` on stdout and a file
//! with no finding prints `FILE: ok`. Exit status 0 means no finding, 1 means
//! a rule failed, 2 means a usage error, an unreadable file or a failed write.

#![forbid(unsafe_code)]

use std::env;
use std::ffi::OsString;
use std::fs;
use std::io::{self, BufWriter, Write};
use std::path::Path;
use std::process::ExitCode;

use elfcheck::{Check, Kind, Machine};

const USAGE: &str = "usage: elfcheck [--machine x86_64|aarch64] [--exec] FILE...\n       \
                     elfcheck --list\n";

/// Checks one file and writes its report; returns the file's exit status.
fn check_file(out: &mut impl Write, path: &Path, kind: Kind, machine: Machine) -> io::Result<u8> {
    let data = match fs::read(path) {
        Ok(data) => data,
        Err(error) => {
            let _ = writeln!(
                io::stderr(),
                "elfcheck: {}: cannot read the file: {error}",
                path.display()
            );
            return Ok(2);
        }
    };
    let report = elfcheck::check_machine(&data, kind, machine);
    for finding in &report.findings {
        writeln!(out, "{}: {finding}", path.display())?;
    }
    if let Some(error) = &report.error {
        let _ = writeln!(io::stderr(), "elfcheck: {}: {error}", path.display());
        return Ok(2);
    }
    if report.findings.is_empty() {
        writeln!(out, "{}: ok", path.display())?;
        return Ok(0);
    }
    Ok(1)
}

fn run(args: &[OsString], out: &mut impl Write) -> io::Result<u8> {
    if args.len() == 1 && args[0] == "--list" {
        for check in Check::ALL {
            writeln!(out, "{}", check.id())?;
        }
        return Ok(0);
    }
    let mut kind = Kind::Relocatable;
    let mut machine = Machine::default();
    let mut files = args;
    loop {
        match files {
            [flag, rest @ ..] if flag == "--exec" && kind == Kind::Relocatable => {
                kind = Kind::Executable;
                files = rest;
            }
            [flag, name, rest @ ..] if flag == "--machine" => {
                let Some(named) = name.to_str().and_then(Machine::named) else {
                    let _ = writeln!(
                        io::stderr(),
                        "elfcheck: unknown machine {name:?}; expected x86_64 or aarch64"
                    );
                    let _ = io::stderr().write_all(USAGE.as_bytes());
                    return Ok(2);
                };
                machine = named;
                files = rest;
            }
            _ => break,
        }
    }
    if files
        .first()
        .is_none_or(|first| first.to_string_lossy().starts_with('-'))
    {
        let _ = io::stderr().write_all(USAGE.as_bytes());
        return Ok(2);
    }
    let mut status = 0;
    for file in files {
        status = status.max(check_file(out, Path::new(file), kind, machine)?);
    }
    Ok(status)
}

fn main() -> ExitCode {
    let args: Vec<OsString> = env::args_os().skip(1).collect();
    let stdout = io::stdout();
    let mut out = BufWriter::new(stdout.lock());
    let status = run(&args, &mut out).and_then(|status| out.flush().map(|()| status));
    match status {
        Ok(status) => ExitCode::from(status),
        Err(error) => {
            let _ = writeln!(io::stderr(), "elfcheck: cannot write the report: {error}");
            ExitCode::from(2)
        }
    }
}
