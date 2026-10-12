//! SCRATCH ONLY, never merged: lock-free event ring for #1401.

use core::sync::atomic::{AtomicU64, Ordering};

const SLOTS: usize = 256;
const WORDS: usize = 10;

static HEAD: AtomicU64 = AtomicU64::new(0);
static PRINTED: AtomicU64 = AtomicU64::new(0);
static RING: [[AtomicU64; WORDS]; SLOTS] = [const { [const { AtomicU64::new(0) }; WORDS] }; SLOTS];
static READY: [AtomicU64; SLOTS] = [const { AtomicU64::new(0) }; SLOTS];

pub const PM_HOLD: u64 = 1;
pub const FBDRAW: u64 = 2;
pub const LATE_WAIT: u64 = 3;
pub const FUTEX_WAKE_SLOW: u64 = 4;
pub const IDLE_GAP: u64 = 5;
pub const POLL_FAIL: u64 = 6;
pub const MUNMAP: u64 = 7;
pub static SHOOTDOWNS: [AtomicU64; 8] = [const { AtomicU64::new(0) }; 8];
static PM_SHOOT0: AtomicU64 = AtomicU64::new(0);
pub static PM_OWNER_TID: AtomicU64 = AtomicU64::new(0);

pub fn now_ns() -> u64 {
    crate::signal::monotonic_nanos()
}

pub fn cpu() -> u64 {
    #[cfg(target_arch = "x86_64")]
    {
        crate::per_cpu::cpu_id() as u64
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        0
    }
}

pub fn push(kind: u64, words: [u64; WORDS - 1]) {
    let seq = HEAD.fetch_add(1, Ordering::AcqRel);
    let slot = (seq as usize) % SLOTS;
    READY[slot].store(0, Ordering::Release);
    RING[slot][0].store(kind, Ordering::Relaxed);
    for (i, w) in words.iter().enumerate() {
        RING[slot][i + 1].store(*w, Ordering::Relaxed);
    }
    READY[slot].store(seq + 1, Ordering::Release);
}

/// Thread context only: print every event pushed since the last call.
pub fn drain() {
    let head = HEAD.load(Ordering::Acquire);
    let mut next = PRINTED.load(Ordering::Acquire);
    if head.saturating_sub(next) > SLOTS as u64 {
        next = head - SLOTS as u64;
    }
    while next < head {
        let slot = (next as usize) % SLOTS;
        if READY[slot].load(Ordering::Acquire) != next + 1 {
            break;
        }
        let mut w = [0u64; WORDS];
        for (i, v) in w.iter_mut().enumerate() {
            *v = RING[slot][i].load(Ordering::Relaxed);
        }
        match w[0] {
            PM_HOLD => {
                let file = unsafe {
                    core::str::from_utf8_unchecked(core::slice::from_raw_parts(w[4] as *const u8, w[5] as usize))
                };
                log::info!(
                    "[P1401 PM_HOLD us={} at_ms={} cpu={} site={}:{} tid={} shootdowns={}]",
                    w[1] / 1000,
                    w[2] / 1_000_000,
                    w[3],
                    file,
                    w[6],
                    w[7],
                    w[8]
                );
            }
            FBDRAW => log::info!(
                "[P1401 FBDRAW us={} at_ms={} cpu={} rows={} cols={}]",
                w[1] / 1000,
                w[2] / 1_000_000,
                w[3],
                w[4],
                w[5]
            ),
            LATE_WAIT => log::info!(
                "[P1401 LATE_WAIT tid={} late_us={} wake_ms={} pop_rel_us={} pop_cpu={} arm={} target={} resume_rel_us={} ret_cpu={}]",
                w[1],
                w[2] / 1000,
                w[3] / 1_000_000,
                w[4] as i64 / 1000,
                w[5],
                w[6],
                w[7],
                w[8] as i64 / 1000,
                w[9]
            ),
            FUTEX_WAKE_SLOW => log::info!(
                "[P1401 FUTEX_WAKE us={} key_us={} at_ms={} cpu={}]",
                w[1] / 1000,
                w[2] / 1000,
                w[3] / 1_000_000,
                w[4]
            ),
            IDLE_GAP => log::info!(
                "[P1401 IDLE_GAP us={} at_ms={} cpu={} switches={}]",
                w[1] / 1000,
                w[2] / 1_000_000,
                w[3],
                w[4]
            ),
            MUNMAP => log::info!(
                "[P1401 MUNMAP us={} at_ms={} cpu={} tid={} len={} pages={} shootdowns={}]",
                w[1] / 1000,
                w[2] / 1_000_000,
                w[3],
                w[4],
                w[5],
                w[6],
                w[7]
            ),
            POLL_FAIL => log::info!("[P1401 POLL_FAIL at_ms={} cpu={} holder_cpu={}]", w[1] / 1_000_000, w[2], w[3]),
            _ => {}
        }
        next += 1;
    }
    PRINTED.store(next, Ordering::Release);
}

// Timer pops, by tid slot: (tid, pop_ns, pass_cpu, arm, target).
static POPS: [[AtomicU64; 5]; 64] = [const { [const { AtomicU64::new(0) }; 5] }; 64];

pub fn note_pop(tid: u64, arm: u64, target: u64) {
    let s = &POPS[(tid % 64) as usize];
    s[0].store(tid, Ordering::Relaxed);
    s[1].store(now_ns(), Ordering::Relaxed);
    s[2].store(cpu(), Ordering::Relaxed);
    s[3].store(arm, Ordering::Relaxed);
    s[4].store(target, Ordering::Relaxed);
}

pub fn pop_of(tid: u64) -> (u64, u64, u64, u64) {
    let s = &POPS[(tid % 64) as usize];
    if s[0].load(Ordering::Relaxed) != tid {
        return (0, 99, 99, 99);
    }
    (
        s[1].load(Ordering::Relaxed),
        s[2].load(Ordering::Relaxed),
        s[3].load(Ordering::Relaxed),
        s[4].load(Ordering::Relaxed),
    )
}

// PM hold tracking: one holder at a time.
static PM_ACQ_NS: AtomicU64 = AtomicU64::new(0);
static PM_FILE: AtomicU64 = AtomicU64::new(0);
static PM_FILE_LEN: AtomicU64 = AtomicU64::new(0);
static PM_LINE: AtomicU64 = AtomicU64::new(0);

pub fn pm_acquired(loc: &'static core::panic::Location<'static>) {
    PM_ACQ_NS.store(now_ns(), Ordering::Relaxed);
    PM_FILE.store(loc.file().as_ptr() as u64, Ordering::Relaxed);
    PM_FILE_LEN.store(loc.file().len() as u64, Ordering::Relaxed);
    PM_LINE.store(loc.line() as u64, Ordering::Relaxed);
    PM_SHOOT0.store(shootdowns_here(), Ordering::Relaxed);
}

pub fn shootdowns_here() -> u64 {
    let c = cpu() as usize;
    if c < 8 { SHOOTDOWNS[c].load(Ordering::Relaxed) } else { 0 }
}

pub fn note_shootdown() {
    let c = cpu() as usize;
    if c < 8 {
        SHOOTDOWNS[c].fetch_add(1, Ordering::Relaxed);
    }
}

pub fn pm_released() {
    let start = PM_ACQ_NS.load(Ordering::Relaxed);
    let held = now_ns().saturating_sub(start);
    if start != 0 && held > 2_000_000 {
        push(
            PM_HOLD,
            [
                held,
                start,
                cpu(),
                PM_FILE.load(Ordering::Relaxed),
                PM_FILE_LEN.load(Ordering::Relaxed),
                PM_LINE.load(Ordering::Relaxed),
                PM_OWNER_TID.load(Ordering::Relaxed),
                shootdowns_here().wrapping_sub(PM_SHOOT0.load(Ordering::Relaxed)),
                0,
            ],
        );
    }
}

// Idle-pass gaps per CPU.
static IDLE_LAST: [AtomicU64; 8] = [const { AtomicU64::new(0) }; 8];
static IDLE_SWITCHES: [AtomicU64; 8] = [const { AtomicU64::new(0) }; 8];
pub static SWITCHES: [AtomicU64; 8] = [const { AtomicU64::new(0) }; 8];

pub fn idle_pass(cpu: usize) {
    if cpu >= 8 {
        return;
    }
    let now = now_ns();
    let last = IDLE_LAST[cpu].swap(now, Ordering::Relaxed);
    let sw = SWITCHES[cpu].load(Ordering::Relaxed);
    let last_sw = IDLE_SWITCHES[cpu].swap(sw, Ordering::Relaxed);
    if last != 0 && now - last > 15_000_000 && sw == last_sw {
        push(IDLE_GAP, [now - last, last, cpu as u64, sw, 0, 0, 0, 0, 0]);
    }
}
