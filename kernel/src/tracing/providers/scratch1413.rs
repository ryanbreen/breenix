//! SCRATCH (never merged): bounded per-victim SIGKILL timeline for #1413/#1414.

use core::sync::atomic::{AtomicU64, Ordering};

const SLOTS: usize = 8;
const EVENTS: usize = 24;

pub const SEND_PUBLISH: u64 = 1;
pub const HOLD_END: u64 = 2;
pub const SELF_DELIVER: u64 = 3;
pub const CLAIM: u64 = 4;
pub const QUARANTINE: u64 = 5;
pub const KICK_PUBLISH: u64 = 6;
pub const DECLINE: u64 = 7;
pub const OBSERVED: u64 = 8;
pub const RETIRE: u64 = 9;
pub const PURGED: u64 = 10;
pub const CLAIM_IN_SEND: u64 = 11;

static PIDS: [AtomicU64; SLOTS] = [const { AtomicU64::new(0) }; SLOTS];
static COUNT: [AtomicU64; SLOTS] = [const { AtomicU64::new(0) }; SLOTS];
static KIND: [[AtomicU64; EVENTS]; SLOTS] = [const { [const { AtomicU64::new(0) }; EVENTS] }; SLOTS];
static TS: [[AtomicU64; EVENTS]; SLOTS] = [const { [const { AtomicU64::new(0) }; EVENTS] }; SLOTS];
static DECLINES: [AtomicU64; SLOTS] = [const { AtomicU64::new(0) }; SLOTS];

fn slot(pid: u64) -> Option<usize> {
    if pid == 0 {
        return None;
    }
    PIDS.iter().position(|p| p.load(Ordering::Acquire) == pid)
}

pub fn track(pid: u64) {
    if slot(pid).is_some() {
        return;
    }
    for p in PIDS.iter() {
        if p.compare_exchange(0, pid, Ordering::AcqRel, Ordering::Acquire).is_ok() {
            return;
        }
    }
}

pub fn tracked(pid: u64) -> bool {
    slot(pid).is_some()
}

#[inline(always)]
fn cpu() -> u64 {
    crate::task::scheduler::current_cpu() as u64
}

/// `arg` is a 32-bit detail (claim result, queue cpu, outcome ...).
pub fn rec(pid: u64, kind: u64, arg: u64) {
    let Some(s) = slot(pid) else { return };
    if kind == DECLINE {
        if DECLINES[s].fetch_add(1, Ordering::AcqRel) >= 2 {
            return;
        }
    }
    let i = COUNT[s].fetch_add(1, Ordering::AcqRel) as usize;
    if i >= EVENTS {
        return;
    }
    TS[s][i].store(crate::tracing::trace_timestamp(), Ordering::Relaxed);
    KIND[s][i].store(kind | (cpu() << 8) | ((arg & 0xffff_ffff) << 16), Ordering::Release);
}

fn name(kind: u64) -> &'static str {
    match kind {
        SEND_PUBLISH => "send_publish",
        HOLD_END => "hold_end",
        SELF_DELIVER => "SELF_DELIVER",
        CLAIM => "claim",
        QUARANTINE => "quarantine",
        KICK_PUBLISH => "kick_publish",
        DECLINE => "decline",
        OBSERVED => "OBSERVED",
        RETIRE => "retire",
        PURGED => "thread_reclaimed(arg=token_queued)",
        CLAIM_IN_SEND => "claim_in_send",
        _ => "?",
    }
}

pub fn dump() {
    for s in 0..SLOTS {
        let pid = PIDS[s].load(Ordering::Acquire);
        if pid == 0 {
            continue;
        }
        let n = (COUNT[s].load(Ordering::Acquire) as usize).min(EVENTS);
        let base = TS[s][0].load(Ordering::Relaxed);
        crate::serial_println!(
            "[S1413] pid={} events={} declines={}",
            pid,
            n,
            DECLINES[s].load(Ordering::Acquire)
        );
        for i in 0..n {
            let k = KIND[s][i].load(Ordering::Acquire);
            let t = TS[s][i].load(Ordering::Relaxed);
            crate::serial_println!(
                "[S1413]   +{} {} cpu={} arg={:#x}",
                t.wrapping_sub(base),
                name(k & 0xff),
                (k >> 8) & 0xff,
                k >> 16
            );
        }
    }
}

/// x86 forcing v4: the victim whose quarantine token goes on the sender's queue.
pub static TOKEN_TO_SENDER: AtomicU64 = AtomicU64::new(0);

pub fn token_target(pid: u64, chosen: usize) -> usize {
    if cfg!(target_arch = "x86_64") && pid != 0 && TOKEN_TO_SENDER.load(Ordering::Acquire) == pid {
        crate::task::scheduler::current_cpu() as usize
    } else {
        chosen
    }
}

/// x86 forcing v4: every CPU but the sender's runs a kernel thread for 30 ms,
/// so no peer's scheduling pass finds its own queue empty and steals.
pub fn start_busy_peers(pid: u64) {
    #[cfg(target_arch = "x86_64")]
    {
        TOKEN_TO_SENDER.store(pid, Ordering::Release);
        let me = crate::task::scheduler::current_cpu() as usize;
        for cpu in 0..16 {
            if cpu != me && crate::arch_impl::x86_64::smp::is_cpu_online(cpu) {
                let _ = crate::task::kthread::kthread_run_on_cpu(
                    move || {
                        let until = crate::signal::monotonic_nanos() + 30_000_000;
                        while crate::signal::monotonic_nanos() < until {
                            crate::task::scheduler::yield_current();
                            core::hint::spin_loop();
                        }
                    },
                    "s1413_busy",
                    cpu,
                );
            }
        }
    }
    #[cfg(not(target_arch = "x86_64"))]
    let _ = pid;
}
