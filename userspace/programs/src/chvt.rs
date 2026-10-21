//! Switch the foreground virtual terminal through the console ABI.
use libbreenix::{error::Error, fs, syscall::{nr, raw}};

const O_NOCTTY: u32 = 0o400;

/// Activate terminal `number` through `path` and wait for it to be the foreground.
fn switch(path: &str, number: u64) -> Result<(), String> {
    let fd = fs::open(path, fs::O_WRONLY | O_NOCTTY).map_err(|e| format!("{path}: {e}"))?;
    let result = [0x5606, 0x5607].into_iter().try_for_each(|request| {
        let ret = unsafe { raw::syscall3(nr::IOCTL, fd.raw(), request, number) };
        Error::from_syscall(ret as i64).map(|_| ()).map_err(|e| format!("{path}: {e}"))
    });
    let _ = fs::close(fd);
    result
}

/// The caller's own virtual terminal first: a user who is not root may switch from
/// the VT they are logged in on, but may not open /dev/tty0, which is root's and the
/// tty group's. Root, or a caller whose controlling terminal is not a VT, uses /dev/tty0.
fn run() -> Result<(), String> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() != 1 { return Err("usage: chvt N".into()); }
    let number: u64 = args[0].parse().map_err(|_| "invalid terminal number")?;
    switch("/dev/tty", number).or_else(|_| switch("/dev/tty0", number))
}

fn main() {
    if let Err(error) = run() { eprintln!("chvt: {error}"); std::process::exit(1); }
}
