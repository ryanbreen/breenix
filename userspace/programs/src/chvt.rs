//! Switch the foreground virtual terminal through the console ABI.
use libbreenix::{error::Error, fs, syscall::{nr, raw}};

fn run() -> Result<(), String> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() != 1 { return Err("usage: chvt N".into()); }
    let number: u64 = args[0].parse().map_err(|_| "invalid terminal number")?;
    let fd = fs::open("/dev/tty0", fs::O_RDWR | 0o400).map_err(|e| e.to_string())?;
    let result = [0x5606, 0x5607].into_iter().try_for_each(|request| {
        let ret = unsafe { raw::syscall3(nr::IOCTL, fd.raw(), request, number) };
        Error::from_syscall(ret as i64).map(|_| ()).map_err(|e| e.to_string())
    });
    let _ = fs::close(fd);
    result
}

fn main() {
    if let Err(error) = run() { eprintln!("chvt: {error}"); std::process::exit(1); }
}
