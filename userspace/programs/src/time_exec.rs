//! Exec target for the Time & timers suite. No serial output: every result goes back to
//! the case through the descriptor named on the command line, or the exit status.
//!
//! `report FD [TIMER [LINGER_MS]]`: write one line to FD, `real=V/I virtual=V/I prof=V/I`,
//! each interval timer's value and interval in microseconds as getitimer reports them
//! (or `error` if getitimer failed), followed by ` timer=R`, the raw return of
//! timer_gettime(TIMER) (0, or a negative errno) when TIMER is given. With LINGER_MS it
//! then sleeps that long, resuming after any interruption, writes `alive` and exits 0;
//! a signal whose default action ends the process ends it first.
use libbreenix::syscall::raw;
use libbreenix::{io, process, types::Fd};

#[cfg(target_arch = "x86_64")]
mod nr {
    pub const NANOSLEEP: u64 = 35;
    pub const GETITIMER: u64 = 36;
    pub const TIMER_GETTIME: u64 = 224;
}
#[cfg(target_arch = "aarch64")]
mod nr {
    pub const NANOSLEEP: u64 = 101;
    pub const GETITIMER: u64 = 102;
    pub const TIMER_GETTIME: u64 = 108;
}

const EINTR: i64 = 4;

fn sys(n: u64, a: [u64; 2]) -> i64 {
    // SAFETY: every caller passes pointers to buffers that live through the call.
    unsafe { raw::syscall2(n, a[0], a[1]) as i64 }
}

fn write_all(fd: Fd, mut bytes: &[u8]) {
    while !bytes.is_empty() {
        match io::write(fd, bytes) {
            Ok(n) if n > 0 => bytes = &bytes[n..],
            _ => process::exit(3),
        }
    }
}

fn itimer(which: u64) -> String {
    let mut cur = [0i64; 4];
    if sys(nr::GETITIMER, [which, cur.as_mut_ptr() as u64]) != 0 {
        return "error".to_string();
    }
    format!("{}/{}", cur[2] * 1_000_000 + cur[3], cur[0] * 1_000_000 + cur[1])
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 || args[1] != "report" {
        process::exit(2);
    }
    let Ok(fd) = args[2].parse::<u64>() else { process::exit(2) };
    let fd = Fd::from_raw(fd);
    let mut line = format!("real={} virtual={} prof={}", itimer(0), itimer(1), itimer(2));
    if let Some(id) = args.get(3) {
        let Ok(id) = id.parse::<i32>() else { process::exit(2) };
        let mut cur = [0i64; 4];
        let ret = sys(nr::TIMER_GETTIME, [id as i64 as u64, cur.as_mut_ptr() as u64]);
        line.push_str(&format!(" timer={ret}"));
    }
    line.push('\n');
    write_all(fd, line.as_bytes());
    if let Some(ms) = args.get(4) {
        let Ok(ms) = ms.parse::<i64>() else { process::exit(2) };
        let mut req = [ms / 1000, (ms % 1000) * 1_000_000];
        loop {
            let mut rem = [0i64; 2];
            let ret = sys(nr::NANOSLEEP, [req.as_ptr() as u64, rem.as_mut_ptr() as u64]);
            if ret != -EINTR {
                break;
            }
            req = rem;
        }
        write_all(fd, b"alive\n");
    }
    process::exit(0)
}
