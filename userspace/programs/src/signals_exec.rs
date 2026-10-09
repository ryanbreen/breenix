//! Exec target for the Signals suite. No serial output: every result goes back to the
//! case through the descriptor named on the command line, or the exit status.
//!
//! - `state FD`: write `key=value` lines to FD describing the signal state exec kept:
//!   `mask`, `pending`, `ign` and `caught` as hexadecimal signal sets (bit N-1 for signal
//!   N; `caught` holds the signals with a handler), and `altstack`, the alternate signal
//!   stack's ss_flags in decimal.
//! - `unblock SIG`: unblock SIG, then exit 0.
use libbreenix::syscall::raw;
use libbreenix::{io, process, types::Fd};
use std::fmt::Write as _;

#[cfg(target_arch = "x86_64")]
mod nr {
    pub const RT_SIGACTION: u64 = 13;
    pub const RT_SIGPROCMASK: u64 = 14;
    pub const RT_SIGPENDING: u64 = 127;
    pub const SIGALTSTACK: u64 = 131;
}
#[cfg(target_arch = "aarch64")]
mod nr {
    pub const RT_SIGACTION: u64 = 134;
    pub const RT_SIGPROCMASK: u64 = 135;
    pub const RT_SIGPENDING: u64 = 136;
    pub const SIGALTSTACK: u64 = 132;
}

const SIG_DFL: u64 = 0;
const SIG_IGN: u64 = 1;
const SIG_BLOCK: u64 = 0;
const SIG_UNBLOCK: u64 = 1;

fn sys(n: u64, a: [u64; 4]) -> i64 {
    // SAFETY: every caller passes pointers to buffers that live through the call.
    unsafe { raw::syscall4(n, a[0], a[1], a[2], a[3]) as i64 }
}

fn write_all(fd: Fd, mut bytes: &[u8]) {
    while !bytes.is_empty() {
        match io::write(fd, bytes) {
            Ok(n) if n > 0 => bytes = &bytes[n..],
            _ => process::exit(3),
        }
    }
}

fn state(fd: Fd) -> ! {
    let mut mask = [0u64];
    let mut pending = [0u64];
    if sys(nr::RT_SIGPROCMASK, [SIG_BLOCK, 0, mask.as_mut_ptr() as u64, 8]) != 0
        || sys(nr::RT_SIGPENDING, [pending.as_mut_ptr() as u64, 8, 0, 0]) != 0
    {
        process::exit(4);
    }
    let (mut ign, mut caught) = (0u64, 0u64);
    for sig in 1..=64u64 {
        // The kernel's struct sigaction starts with the handler, whatever order the rest takes.
        let mut old = [0u64; 4];
        if sys(nr::RT_SIGACTION, [sig, 0, old.as_mut_ptr() as u64, 8]) != 0 { continue; }
        match old[0] {
            SIG_DFL => {}
            SIG_IGN => ign |= 1 << (sig - 1),
            _ => caught |= 1 << (sig - 1),
        }
    }
    // stack_t: ss_sp, ss_flags (int, padded), ss_size.
    let mut stack = [0u64; 3];
    let altstack = if sys(nr::SIGALTSTACK, [0, stack.as_mut_ptr() as u64, 0, 0]) == 0 {
        (stack[1] as u32 as i32).to_string()
    } else {
        "error".to_string()
    };
    let mut out = String::new();
    let _ = write!(out, "mask={:x}\npending={:x}\nign={ign:x}\ncaught={caught:x}\naltstack={altstack}\n", mask[0], pending[0]);
    write_all(fd, out.as_bytes());
    process::exit(0)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let number = |i: usize| args.get(i).and_then(|s| s.parse::<u64>().ok()).unwrap_or_else(|| process::exit(2));
    match args.get(1).map(String::as_str) {
        Some("state") => state(Fd::from_raw(number(2))),
        Some("unblock") => {
            let set = [1u64 << (number(2) - 1)];
            if sys(nr::RT_SIGPROCMASK, [SIG_UNBLOCK, set.as_ptr() as u64, 0, 8]) != 0 { process::exit(5); }
            process::exit(0)
        }
        _ => process::exit(2),
    }
}
