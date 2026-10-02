//! Exec-side descriptor check for the Files & I/O suite. No serial output.
use libbreenix::{error::Error, fs, io, process, types::Fd, Errno};
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let Some(fd) = args.get(1).and_then(|s| s.parse::<u64>().ok()) else {
        process::exit(2)
    };
    let d = Fd::from_raw(fd);
    let good = match args.get(2).map(String::as_str) {
        Some("closed") => matches!(fs::fstat(d), Err(Error::Os(Errno::EBADF))),
        Some("open") => {
            let mut b = [0; 6];
            matches!(io::read(d, &mut b), Ok(6)) && &b == b"abcdef"
        }
        _ => false,
    };
    process::exit(if good { 0 } else { 1 });
}
