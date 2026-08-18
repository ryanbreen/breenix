//! #575 forced-race oracle: a published block request must survive a signal.
//!
//! The #575 failure needs a signal to land on a thread that is already parked
//! inside the virtio-blk completion wait for a request the driver has already
//! published to the device. Waiting for that to happen by luck is not an
//! oracle — the trigger rate is build-timing sensitive — so this program
//! forces the race in real process context with no driver hook and no fake
//! completion. Both stages fork a child that sleeps briefly and exits without
//! being reaped, read one large file across the child's exit, then read a
//! second large file as the leaked-gate probe:
//!
//!   stage 1  leaves SIGCHLD at its default Ignore disposition. This reproduces
//!            the original #575 trigger, where either the signal-disposition
//!            filter or the driver's uninterruptible wait can prevent damage.
//!   stage 2  installs a real SIGCHLD handler before repeating the race. This
//!            removes the signal-filter barrier from the picture, so the
//!            driver's uninterruptible wait is the only thing standing between
//!            the test and a corrupted published request.
//!
//! Both outcomes are printed as a single pinned marker line. The line is
//! emitted twice because console output interleaves at byte granularity, so a
//! single copy can shred; the gate accepts one intact copy and treats the
//! total absence of the marker as a FAIL, never as a skip.

use std::sync::atomic::{AtomicBool, Ordering};

use libbreenix::fs;
use libbreenix::io;
use libbreenix::process::{self, ForkResult};
use libbreenix::signal::SIGCHLD;
use libbreenix::time;
use libbreenix::{sigaction, Sigaction};

/// First large file: the ELF whose load is where #575 was originally observed.
const FILE_A: &str = "/bin/bwm";
/// Second, distinct large file — the leaked-gate probe.
const FILE_B: &str = "/bin/bsshd";
/// Both files must be large enough that the read cannot complete inside the
/// child's sleep window.
const MIN_BYTES: u64 = 256 * 1024;
/// Child sleep before exit. Short enough that the signal lands well inside
/// the first read, long enough that the read is already inside the driver.
const CHILD_SLEEP_MS: u64 = 120;
/// Read chunk. Small enough that many read syscalls span the child's exit.
const CHUNK: usize = 32 * 1024;
/// In-process bound for each leaked-gate probe. A leaked gate parks forever and
/// is caught by the harness timeout instead; this catches a pathological read.
const PROBE_DEADLINE_MS: u64 = 30_000;

static SIGCHLD_HANDLED: AtomicBool = AtomicBool::new(false);

extern "C" fn sigchld_handler(_signal: i32) {
    SIGCHLD_HANDLED.store(true, Ordering::SeqCst);
}

fn monotonic_ms() -> u64 {
    match time::now_monotonic() {
        Ok(ts) => (ts.tv_sec as u64) * 1000 + (ts.tv_nsec as u64) / 1_000_000,
        Err(_) => 0,
    }
}

struct Failure {
    stage: &'static str,
    detail: String,
}

fn fail(stage: &'static str, detail: String) -> Failure {
    Failure { stage, detail }
}

struct ReadStages {
    open: &'static str,
    fstat: &'static str,
    read: &'static str,
}

struct RaceStages {
    fork: &'static str,
    first: ReadStages,
    small1: &'static str,
    short1: &'static str,
    second: ReadStages,
    small2: &'static str,
    short2: &'static str,
    slow2: &'static str,
}

/// Read `path` to EOF, returning `(bytes_read, stat_size)`.
fn read_whole(path: &str, stages: &ReadStages) -> Result<(u64, u64), Failure> {
    let fd = match fs::open(path, fs::O_RDONLY) {
        Ok(fd) => fd,
        Err(e) => return Err(fail(stages.open, format!("{}", e))),
    };

    let size = match fs::fstat(fd) {
        Ok(stat) => stat.st_size as u64,
        Err(e) => {
            let _ = io::close(fd);
            return Err(fail(stages.fstat, format!("{}", e)));
        }
    };

    let mut buf = vec![0u8; CHUNK];
    let mut total: u64 = 0;
    loop {
        match fs::read(fd, &mut buf) {
            Ok(0) => break,
            Ok(n) => total += n as u64,
            Err(e) => {
                let _ = io::close(fd);
                return Err(fail(stages.read, format!("{}", e)));
            }
        }
    }
    let _ = io::close(fd);
    Ok((total, size))
}

fn emit(line: &str) {
    // Emitted twice: a shredded copy must not be able to hide the verdict.
    print!("{}\n", line);
    print!("{}\n", line);
}

fn run_race(stages: &RaceStages) -> Result<(), Failure> {
    match process::fork() {
        Ok(ForkResult::Child) => {
            let _ = time::sleep_ms(CHILD_SLEEP_MS);
            process::exit(0);
        }
        Ok(ForkResult::Parent(_pid)) => {}
        Err(e) => return Err(fail(stages.fork, format!("{}", e))),
    }

    let (got_a, size_a) = read_whole(FILE_A, &stages.first)?;
    if size_a < MIN_BYTES {
        return Err(fail(stages.small1, format!("{}", size_a)));
    }
    if got_a != size_a {
        return Err(fail(stages.short1, format!("{}of{}", got_a, size_a)));
    }

    let started_ms = monotonic_ms();
    let (got_b, size_b) = read_whole(FILE_B, &stages.second)?;
    let elapsed_ms = monotonic_ms().saturating_sub(started_ms);
    if size_b < MIN_BYTES {
        return Err(fail(stages.small2, format!("{}", size_b)));
    }
    if got_b != size_b {
        return Err(fail(stages.short2, format!("{}of{}", got_b, size_b)));
    }
    if elapsed_ms > PROBE_DEADLINE_MS {
        return Err(fail(stages.slow2, format!("{}ms", elapsed_ms)));
    }

    Ok(())
}

fn run() -> Result<(), Failure> {
    // Stage 1 — retain SIGCHLD's default Ignore disposition.
    run_race(&RaceStages {
        fork: "ign_fork",
        first: ReadStages {
            open: "ign_open1",
            fstat: "ign_fstat1",
            read: "ign_read1",
        },
        small1: "ign_small1",
        short1: "ign_short1",
        second: ReadStages {
            open: "ign_open2",
            fstat: "ign_fstat2",
            read: "ign_read2",
        },
        small2: "ign_small2",
        short2: "ign_short2",
        slow2: "ign_slow2",
    })?;

    // Stage 2 — a handled SIGCHLD makes the driver's wait the only barrier.
    let action = Sigaction::new(sigchld_handler);
    if let Err(e) = sigaction(SIGCHLD, Some(&action), None) {
        return Err(fail("sigaction", format!("{}", e)));
    }
    SIGCHLD_HANDLED.store(false, Ordering::SeqCst);

    run_race(&RaceStages {
        fork: "sig_fork",
        first: ReadStages {
            open: "sig_open1",
            fstat: "sig_fstat1",
            read: "sig_read1",
        },
        small1: "sig_small1",
        short1: "sig_short1",
        second: ReadStages {
            open: "sig_open2",
            fstat: "sig_fstat2",
            read: "sig_read2",
        },
        small2: "sig_small2",
        short2: "sig_short2",
        slow2: "sig_slow2",
    })?;

    if !SIGCHLD_HANDLED.load(Ordering::SeqCst) {
        return Err(fail("sig_handler_never_ran", "flag=0".to_string()));
    }

    Ok(())
}

fn main() {
    match run() {
        Ok(()) => {
            emit("[BLOCK_EINTR_ORACLE:PASS:stages=2:reads=4:short=0:eintr=0:handled=1]");
            process::exit(0);
        }
        Err(f) => {
            emit(&format!(
                "[BLOCK_EINTR_ORACLE:FAIL:{}:{}]",
                f.stage, f.detail
            ));
            process::exit(1);
        }
    }
}
