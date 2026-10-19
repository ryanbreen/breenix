//! #575 forced-race oracle: a published block request must survive a signal.
//!
//! The #575 failure needs a signal to land on a thread that is already parked
//! inside the virtio-blk completion wait for a request the driver has already
//! published to the device. Waiting for that to happen by luck is not an
//! oracle — the trigger rate is build-timing sensitive — so this program
//! attempts the race in real process context with no driver hook and no fake
//! completion, then reads a second large file as the leaked-gate probe:
//!
//!   stage 1  leaves SIGCHLD at its default Ignore disposition. This reproduces
//!            the original #575 trigger, where either the signal-disposition
//!            filter or the driver's uninterruptible wait can prevent damage.
//!            Its child sleeps briefly and exits during the read.
//!   stage 2  installs a real SIGCHLD handler before repeating the race. This
//!            removes the signal-filter barrier from the picture, so the
//!            driver's uninterruptible wait is the only thing standing between
//!            the test and a corrupted published request.
//!
//! Stage 2 observes the overlap in the reader's scheduler state. Its child
//! samples `/proc/<pid>/stat` during the read loop and exits only after it
//! reads `D`: procfs reports `D` only for a thread sleeping in a completion
//! wait that signals cannot end, and the one such wait a file read reaches is
//! the block driver's, entered after the request is published to the device.
//! The sample is taken before the child exits, so before SIGCHLD is sent, and
//! it counts only if the reads had not ended when it was taken.
//! The handler records the sender's si_pid, which must be the child's, and
//! must run before the reads end. An attempt that misses either repeats with
//! a longer hold, up to the last, and a race the last hold misses is a FAIL.
//!
//! Both outcomes are printed as a single pinned marker line. The line is
//! emitted twice because console output interleaves at byte granularity, so a
//! single copy can shred; the gate accepts one intact copy and treats the
//! total absence of the marker as a FAIL, never as a skip.

use std::sync::atomic::{AtomicI32, Ordering};

use libbreenix::errno::Errno;
use libbreenix::error::Error;
use libbreenix::fs;
use libbreenix::io;
use libbreenix::process::{self, ForkResult};
use libbreenix::signal::{__restore_rt, SA_RESTORER, SA_SIGINFO, SIGCHLD};
use libbreenix::time;
use libbreenix::types::{Fd, Pid};
use libbreenix::{sigaction, Sigaction};

/// First large file: the ELF whose load is where #575 was originally observed.
const FILE_A: &str = "/bin/bwm";
/// Second, distinct large file — the leaked-gate probe.
const FILE_B: &str = "/bin/bsshd";
/// Minimum payload size; overlap with the child's exit is not implied by it.
const MIN_BYTES: u64 = 256 * 1024;
/// Stage 1's child sleep before exit.
const CHILD_SLEEP_MS: u64 = 120;
/// Stage 2's passes over the file per attempt: each attempt that does not
/// exercise the race holds the read open for longer, up to the last.
const HOLDS: [u32; 4] = [1, 2, 4, 8];
/// Stage 2 child exit codes: it saw the reader parked on a block request and
/// exited at once; the reads ended before it saw that; it could not observe
/// the reader.
const CHILD_SAW_PARKED: i32 = 0;
const CHILD_NOT_PARKED: i32 = 3;
const CHILD_NO_OBSERVATION: i32 = 4;
/// Bound both the poll count and monotonic elapsed time for child reaping.
const CHILD_WAIT_POLLS: usize = 5_000;
const CHILD_WAIT_MS: u64 = 5_000;
/// Read chunk. Small enough that many read syscalls span the child's exit.
const CHUNK: usize = 32 * 1024;
/// In-process bound for each leaked-gate probe. A leaked gate parks forever and
/// is caught by the harness timeout instead; this catches a pathological read.
const PROBE_DEADLINE_MS: u64 = 30_000;

/// The si_pid of the last SIGCHLD handled, or 0.
static SIGCHLD_PID: AtomicI32 = AtomicI32::new(0);

extern "C" fn sigchld_handler(_signal: i32, info: *const u8, _context: *const u8) {
    // si_pid is bytes 16..20 of the Linux siginfo_t.
    // SAFETY: with SA_SIGINFO the kernel passes the siginfo in the frame.
    let pid = unsafe { core::ptr::read_volatile(info.add(16) as *const i32) };
    SIGCHLD_PID.store(pid, Ordering::SeqCst);
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

fn run_race(stages: &RaceStages) -> Result<Pid, Failure> {
    let child = match process::fork() {
        Ok(ForkResult::Child) => {
            let _ = time::sleep_ms(CHILD_SLEEP_MS);
            process::exit(0);
        }
        Ok(ForkResult::Parent(pid)) => pid,
        Err(e) => return Err(fail(stages.fork, format!("{}", e))),
    };

    let (got_a, size_a) = read_whole(FILE_A, &stages.first)?;
    if size_a < MIN_BYTES {
        return Err(fail(stages.small1, format!("{}", size_a)));
    }
    if got_a != size_a {
        return Err(fail(stages.short1, format!("{}of{}", got_a, size_a)));
    }
    probe(stages)?;
    Ok(child)
}

/// The leaked-gate probe: a second large file must read whole, in time.
fn probe(stages: &RaceStages) -> Result<(), Failure> {
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

/// Poll `done` until it holds, bounded by both a poll count and a monotonic
/// deadline. False when the bound passed first.
fn bounded(stage: &'static str, mut done: impl FnMut() -> Result<bool, Failure>) -> Result<bool, Failure> {
    let clock_ms = || {
        time::now_monotonic()
            .map(|ts| ts.tv_sec as u64 * 1000 + ts.tv_nsec as u64 / 1_000_000)
            .map_err(|e| fail(stage, format!("clock:{}", e)))
    };
    let deadline = clock_ms()?.saturating_add(CHILD_WAIT_MS);
    for _ in 0..CHILD_WAIT_POLLS {
        if done()? {
            return Ok(true);
        }
        if clock_ms()? >= deadline {
            break;
        }
        let _ = time::sleep_ms(1);
    }
    Ok(false)
}

/// Reap `child`, which must exit normally; returns its exit code.
fn wait_child(child: Pid, stage: &'static str) -> Result<i32, Failure> {
    let mut code = None;
    let exited = bounded(stage, || {
        let mut status = 0;
        match process::waitpid(child.raw() as i32, &mut status, process::WNOHANG) {
            Ok(pid) if pid == child => {
                if !process::wifexited(status) {
                    return Err(fail(stage, format!("status={}", status)));
                }
                code = Some(process::wexitstatus(status));
                Ok(true)
            }
            Ok(_) | Err(Error::Os(Errno::EINTR)) => Ok(false),
            Err(e) => Err(fail(stage, format!("{}", e))),
        }
    })?;
    match code {
        Some(code) if exited => Ok(code),
        _ => Err(fail(stage, "child_exit_timeout".to_string())),
    }
}

/// The reader's scheduler state letter from `/proc/<pid>/stat`. `D` is a
/// thread parked in a completion wait that signals cannot end.
fn stat_state(path: &str, buf: &mut [u8]) -> Option<u8> {
    let fd = fs::open(path, fs::O_RDONLY).ok()?;
    let n = fs::read(fd, buf);
    let _ = io::close(fd);
    let line = &buf[..n.ok()?];
    // "pid (comm) state ...": the command name may hold any byte but ')'
    // ends it at its last occurrence.
    let close = line.iter().rposition(|&b| b == b')')?;
    line.get(close + 2).copied()
}

/// Stage 2's child: wait for the reader's go byte, then exit after observing
/// the reader parked on a published block request (`D`) during the read loop.
fn watch_reader(reader: Pid, ctl: Fd) -> i32 {
    let mut byte = [0u8; 1];
    loop {
        match io::read(ctl, &mut byte) {
            Ok(1) => break,
            Ok(_) => return CHILD_NOT_PARKED,
            Err(Error::Os(Errno::EINTR)) => {}
            Err(_) => return CHILD_NO_OBSERVATION,
        }
    }
    if io::fcntl_setfl(ctl, io::status_flags::O_NONBLOCK).is_err() {
        return CHILD_NO_OBSERVATION;
    }
    let path = format!("/proc/{}/stat", reader.raw());
    let mut stat = [0u8; 256];
    loop {
        let state = match stat_state(&path, &mut stat) {
            Some(state) => state,
            None => return CHILD_NO_OBSERVATION,
        };
        // Read the control pipe after the state: a parked state counts only
        // if the reader had not yet ended its reads when it was sampled.
        match io::read(ctl, &mut byte) {
            Err(Error::Os(Errno::EAGAIN)) | Err(Error::Os(Errno::EINTR)) => {}
            Ok(_) => return CHILD_NOT_PARKED,
            Err(_) => return CHILD_NO_OBSERVATION,
        }
        if state == b'D' {
            return CHILD_SAW_PARKED;
        }
    }
}

const SIG_STAGES: RaceStages = RaceStages {
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
};

/// Read FILE_A `hold` times with the watcher told when the reads begin and
/// end. Returns whether the watcher's SIGCHLD was handled before they ended.
fn held_reads(child: Pid, hold: u32, ctl: Fd) -> Result<bool, Failure> {
    let stages = &SIG_STAGES.first;
    let fd = fs::open(FILE_A, fs::O_RDONLY).map_err(|e| fail(stages.open, format!("{}", e)))?;
    let result = (|| {
        let size = fs::fstat(fd).map_err(|e| fail(stages.fstat, format!("{}", e)))?.st_size as u64;
        if size < MIN_BYTES {
            return Err(fail(SIG_STAGES.small1, format!("{}", size)));
        }
        let mut buf = vec![0xa5u8; CHUNK];
        if io::write(ctl, b"g").is_err() {
            return Err(fail("sig_ctl", "go".to_string()));
        }
        for _ in 0..hold {
            fs::lseek(fd, 0, fs::SEEK_SET).map_err(|e| fail(stages.read, format!("seek:{}", e)))?;
            let mut total: u64 = 0;
            loop {
                match fs::read(fd, &mut buf) {
                    Ok(0) => break,
                    Ok(n) => total += n as u64,
                    Err(e) => return Err(fail(stages.read, format!("{}", e))),
                }
            }
            if total != size {
                return Err(fail(SIG_STAGES.short1, format!("{}of{}", total, size)));
            }
        }
        Ok(SIGCHLD_PID.load(Ordering::SeqCst) == child.raw() as i32)
    })();
    let _ = io::close(fd);
    result
}

enum Attempt {
    Exercised,
    NotParked,
    HandledAfterReads,
}

fn parked_attempt(reader: Pid, hold: u32) -> Result<Attempt, Failure> {
    let (ctl_r, ctl_w) = io::pipe().map_err(|e| fail("sig_pipe", format!("{}", e)))?;
    SIGCHLD_PID.store(0, Ordering::SeqCst);
    let child = match process::fork() {
        Ok(ForkResult::Child) => {
            let _ = io::close(ctl_w);
            process::exit(watch_reader(reader, ctl_r));
        }
        Ok(ForkResult::Parent(pid)) => pid,
        Err(e) => return Err(fail(SIG_STAGES.fork, format!("{}", e))),
    };
    // The reader keeps its read end until the reads end, so a write to the
    // pipe finds a reader even after the child has exited.
    let reads = held_reads(child, hold, ctl_w);
    let _ = io::close(ctl_w);
    let _ = io::close(ctl_r);
    let code = wait_child(child, "sig_waitpid")?;
    let handled_during_reads = reads?;
    if code == CHILD_NO_OBSERVATION {
        return Err(fail("sig_observe", format!("hold={}", hold)));
    }
    if code != CHILD_SAW_PARKED && code != CHILD_NOT_PARKED {
        return Err(fail("sig_waitpid", format!("code={}", code)));
    }
    // The child's exit sends SIGCHLD: wait for its handler.
    let mut sender = 0;
    let handled = bounded("sig_waitpid", || {
        sender = SIGCHLD_PID.load(Ordering::SeqCst);
        Ok(sender == child.raw() as i32)
    })?;
    if !handled {
        return Err(fail("sig_handler_never_ran", format!("si_pid={}", sender)));
    }
    Ok(if code == CHILD_NOT_PARKED {
        Attempt::NotParked
    } else if !handled_during_reads {
        Attempt::HandledAfterReads
    } else {
        Attempt::Exercised
    })
}

fn run() -> Result<u32, Failure> {
    // Stage 1 — retain SIGCHLD's default Ignore disposition.
    let ignored_child = run_race(&RaceStages {
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

    // Reap stage 1 before installing the handler so it cannot satisfy stage 2.
    let code = wait_child(ignored_child, "ign_waitpid")?;
    if code != 0 {
        return Err(fail("ign_waitpid", format!("code={}", code)));
    }

    // Stage 2 — a handled SIGCHLD makes the driver's wait the only barrier.
    let action = Sigaction {
        handler: sigchld_handler as usize as u64,
        flags: SA_RESTORER | SA_SIGINFO,
        restorer: __restore_rt as usize as u64,
        mask: 0,
    };
    if let Err(e) = sigaction(SIGCHLD, Some(&action), None) {
        return Err(fail("sigaction", format!("{}", e)));
    }
    let reader = process::getpid().map_err(|e| fail("sig_getpid", format!("{}", e)))?;

    let mut missed = "not_parked";
    for hold in HOLDS {
        match parked_attempt(reader, hold)? {
            Attempt::Exercised => {
                probe(&SIG_STAGES)?;
                return Ok(hold);
            }
            Attempt::NotParked => missed = "not_parked",
            Attempt::HandledAfterReads => missed = "handled_after_reads",
        }
    }
    Err(fail(
        "sig_race_not_exercised",
        format!("{}:hold={}", missed, HOLDS[HOLDS.len() - 1]),
    ))
}

fn main() {
    match run() {
        Ok(hold) => {
            emit(&format!(
                "[BLOCK_EINTR_ORACLE:PASS:stages=2:reads=4:short=0:eintr=0:handled=1:parked=1:hold={}]",
                hold
            ));
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
