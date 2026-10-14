//! Program the Rust std suite runs through std::process::Command. It uses std only, and
//! prints nothing a suite reader takes for a record: its results go to the stdout and
//! stderr its parent gave it, or into its exit status.
//!
//! - `args [ARG...]`: write each argument's bytes, from args_os, followed by a newline.
//! - `argv0`: write argv[0]'s bytes and a newline.
//! - `allargs`: write every env::args string, argv[0] first, one per line.
//! - `env NAME...`: write `NAME=value` for each variable that is set, `NAME unset` otherwise.
//! - `envall`: write `NAME=value` for every variable, one per line, sorted.
//! - `cwd`: write env::current_dir.
//! - `exe`: write env::current_exe.
//! - `ids`: write process::id and os::unix::process::parent_id, separated by a space.
//! - `exit CODE`: process::exit(CODE).
//! - `abort`: process::abort.
//! - `cat`: copy stdin to stdout with io::copy, then exit 0.
//! - `split`: write `out` to stdout and `err` to stderr.
//! - `sleep MS`: sleep MS milliseconds, then exit 0.
//! - `thread-panic`: panic in a spawned thread and join it; under panic=abort the process
//!   dies of SIGABRT before the join returns, otherwise it exits 3.
//! - `panic-hook`: set a panic hook that writes `hooked` to stdout, then panic.
use std::io::{self, Write};
use std::os::unix::ffi::OsStrExt;

fn line(bytes: &[u8]) {
    let mut out = io::stdout().lock();
    let _ = out.write_all(bytes);
    let _ = out.write_all(b"\n");
    let _ = out.flush();
}

fn main() {
    let args: Vec<std::ffi::OsString> = std::env::args_os().collect();
    let mode = args.get(1).map(|arg| arg.as_bytes().to_vec()).unwrap_or_default();
    let rest = args.get(2..).unwrap_or(&[]);
    let first = || rest.first().and_then(|arg| arg.to_str()).unwrap_or("");
    match &mode[..] {
        b"args" => rest.iter().for_each(|arg| line(arg.as_bytes())),
        b"argv0" => line(args.first().map(|arg| arg.as_bytes()).unwrap_or(b"")),
        b"allargs" => std::env::args().for_each(|arg| line(arg.as_bytes())),
        b"env" => {
            for name in rest {
                match std::env::var_os(name) {
                    Some(value) => line(&[name.as_bytes(), b"=", value.as_bytes()].concat()),
                    None => line(&[name.as_bytes(), b" unset"].concat()),
                }
            }
        }
        b"envall" => {
            let mut vars: Vec<Vec<u8>> = std::env::vars_os()
                .map(|(name, value)| [name.as_bytes(), b"=", value.as_bytes()].concat())
                .collect();
            vars.sort();
            vars.iter().for_each(|var| line(var));
        }
        b"cwd" => match std::env::current_dir() {
            Ok(dir) => line(dir.as_os_str().as_bytes()),
            Err(error) => std::process::exit(error.raw_os_error().unwrap_or(99).min(99) + 100),
        },
        b"exe" => match std::env::current_exe() {
            Ok(path) => line(path.as_os_str().as_bytes()),
            Err(error) => std::process::exit(error.raw_os_error().unwrap_or(99).min(99) + 100),
        },
        b"ids" => line(format!("{} {}", std::process::id(), std::os::unix::process::parent_id()).as_bytes()),
        b"exit" => std::process::exit(first().parse().unwrap_or(99)),
        b"abort" => std::process::abort(),
        b"cat" => {
            if io::copy(&mut io::stdin().lock(), &mut io::stdout().lock()).is_err() {
                std::process::exit(4);
            }
        }
        b"split" => {
            let _ = io::stdout().write_all(b"out");
            let _ = io::stderr().write_all(b"err");
        }
        b"sleep" => std::thread::sleep(std::time::Duration::from_millis(first().parse().unwrap_or(0))),
        b"thread-panic" => {
            let _ = std::thread::spawn(|| panic!("rust-std suite: deliberate panic")).join();
            std::process::exit(3);
        }
        b"panic-hook" => {
            std::panic::set_hook(Box::new(|_| line(b"hooked")));
            panic!("rust-std suite: deliberate panic");
        }
        _ => std::process::exit(98),
    }
}
