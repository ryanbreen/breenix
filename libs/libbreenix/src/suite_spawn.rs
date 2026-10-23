//! `suite::spawn`: how a suite case creates a helper process (`docs/gui/plan.md` section 8.2).
//!
//! The child closes every descriptor it inherited from the runner and the case, keeping only
//! the ones the caller hands it (moved to the numbers it names), then runs a closure or execs a
//! program. So a helper never holds the runner's report, record or live pipes, whether or not it
//! execs, and the descriptors it does hold are the ones its case chose.

use crate::error::Error;
use crate::io;
use crate::process::{self, ForkResult};
use crate::syscall::{nr, raw};
use crate::types::{Fd, Pid};

/// What the child runs once its descriptors are set up.
pub enum Run<'a> {
    /// A closure; its result is the child's exit status.
    Call(&'a mut dyn FnMut() -> i32),
    /// `execv(path, argv)`; the child exits with 127 if the exec fails. Each string ends in a
    /// NUL byte.
    Exec(&'a [u8], &'a [&'a [u8]]),
}

/// Descriptors the child keeps: `(from, to)` moves the caller's `from` to `to` in the child.
pub type Keep<'a> = &'a [(Fd, Fd)];

/// The soft `RLIMIT_NOFILE`: every descriptor the process can hold is below it.
fn descriptor_limit() -> u64 {
    let mut limit = [0u64; 2];
    // SAFETY: prlimit64(0, RLIMIT_NOFILE, NULL, &limit) writes two u64s into `limit`.
    let ret = unsafe { raw::syscall4(nr::PRLIMIT64, 0, 7, 0, limit.as_mut_ptr() as u64) } as i64;
    if ret < 0 || limit[0] == 0 {
        4096
    } else {
        limit[0].min(4096)
    }
}

/// In the child: put each kept descriptor at its number and close every other one from 3 up.
/// Standard input, output and error stay unless a keep entry replaces them.
fn arrange(keep: Keep<'_>) -> Result<(), Error> {
    // Move the kept descriptors above every target first, so no move overwrites one still
    // to be moved.
    let top = descriptor_limit();
    let staged: std::vec::Vec<(Fd, Fd)> = keep
        .iter()
        .enumerate()
        .map(|(i, &(from, to))| {
            let parked = Fd::from_raw(top - 1 - i as u64);
            io::dup2(from, parked).map(|p| (p, to))
        })
        .collect::<Result<_, _>>()?;
    for &(parked, to) in staged.iter() {
        io::dup2(parked, to)?;
    }
    let targets: std::vec::Vec<u64> = keep.iter().map(|&(_, to)| to.raw()).collect();
    for fd in 3..top {
        if !targets.contains(&fd) {
            let _ = io::close(Fd::from_raw(fd));
        }
    }
    Ok(())
}

/// Create a helper process: fork, keep only `keep` (each moved to its target), then `run`.
/// Returns the child's pid in the parent.
pub fn spawn(keep: Keep<'_>, run: Run<'_>) -> Result<Pid, Error> {
    match process::fork()? {
        ForkResult::Parent(pid) => Ok(pid),
        ForkResult::Child => {
            if arrange(keep).is_err() {
                process::exit(126);
            }
            let status = match run {
                Run::Call(f) => f(),
                Run::Exec(path, argv) => {
                    let mut ptrs: std::vec::Vec<*const u8> = argv.iter().map(|a| a.as_ptr()).collect();
                    ptrs.push(core::ptr::null());
                    let _ = process::execv(path, ptrs.as_ptr());
                    127
                }
            };
            process::exit(status)
        }
    }
}
