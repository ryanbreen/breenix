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
pub const HOUSEKEEP: u64 = 8;
pub const TIMED: u64 = 9;
pub const POST_EXIT: u64 = 10;
pub const STAGES: u64 = 11;
pub const IPI_SLOW: u64 = 12;
pub const NMI_SAMPLE: u64 = 13;
pub const NMI_STACK: u64 = 14;
// Page faults per thread.
static PF_TID: [AtomicU64; 32] = [const { AtomicU64::new(0) }; 32];
static PF_N: [AtomicU64; 32] = [const { AtomicU64::new(0) }; 32];
static PF_CR2: [AtomicU64; 32] = [const { AtomicU64::new(0) }; 32];
static PF_RIP: [AtomicU64; 32] = [const { AtomicU64::new(0) }; 32];
static PF_ERR: [AtomicU64; 32] = [const { AtomicU64::new(0) }; 32];
pub fn page_fault(tid: u64, cr2: u64, rip: u64, err: u64) {
    let mut i = (tid as usize) % 32;
    for _ in 0..32 {
        let t = PF_TID[i].load(Ordering::Relaxed);
        if t == tid || (t == 0 && PF_TID[i].compare_exchange(0, tid, Ordering::Relaxed, Ordering::Relaxed).is_ok()) {
            PF_N[i].fetch_add(1, Ordering::Relaxed);
            PF_CR2[i].store(cr2, Ordering::Relaxed);
            PF_RIP[i].store(rip, Ordering::Relaxed);
            PF_ERR[i].store(err, Ordering::Relaxed);
            return;
        }
        i = (i + 1) % 32;
    }
}
// PM occupancy by acquisition line (16 slots, by line % 16 collisions tolerated).
static OCC_LINE: [AtomicU64; 16] = [const { AtomicU64::new(0) }; 16];
static OCC_NS: [AtomicU64; 16] = [const { AtomicU64::new(0) }; 16];
static OCC_N: [AtomicU64; 16] = [const { AtomicU64::new(0) }; 16];
fn occ(line: u64, ns: u64) {
    let mut i = (line as usize) % 16;
    for _ in 0..16 {
        let l = OCC_LINE[i].load(Ordering::Relaxed);
        if l == line || (l == 0 && OCC_LINE[i].compare_exchange(0, line, Ordering::Relaxed, Ordering::Relaxed).is_ok()) {
            OCC_NS[i].fetch_add(ns, Ordering::Relaxed);
            OCC_N[i].fetch_add(1, Ordering::Relaxed);
            return;
        }
        i = (i + 1) % 16;
    }
}
pub static BODY_N: AtomicU64 = AtomicU64::new(0);
pub static BODY_TOTAL: AtomicU64 = AtomicU64::new(0);
pub static BODY_MAX: AtomicU64 = AtomicU64::new(0);
pub static BODY_GT1: AtomicU64 = AtomicU64::new(0);
pub static THREADS_LEN: AtomicU64 = AtomicU64::new(0);
pub fn body(ns: u64) {
    BODY_N.fetch_add(1, Ordering::Relaxed);
    BODY_TOTAL.fetch_add(ns, Ordering::Relaxed);
    BODY_MAX.fetch_max(ns, Ordering::Relaxed);
    if ns > 1_000_000 {
        BODY_GT1.fetch_add(1, Ordering::Relaxed);
    }
}
static SAMPLE_REQ: [AtomicU64; 8] = [const { AtomicU64::new(0) }; 8];
static SAMPLE_WHY: [AtomicU64; 8] = [const { AtomicU64::new(0) }; 8];
static SAMPLE_FROM: [AtomicU64; 8] = [const { AtomicU64::new(0) }; 8];
pub static SCHED_OWNER: AtomicU64 = AtomicU64::new(u64::MAX);
pub static SCHED_ACQ: AtomicU64 = AtomicU64::new(0);

pub fn sched_acquired() {
    SCHED_OWNER.store(cpu(), Ordering::Relaxed);
    SCHED_ACQ.store(now_ns(), Ordering::Relaxed);
}

/// Ask `target` (the holder of a lock this CPU has waited on) where it is, by NMI.
#[cfg(target_arch = "x86_64")]
pub fn request_sample(target: u64, why: u64) {
    let t = target as usize;
    if t >= 8 || target == cpu() {
        return;
    }
    if SAMPLE_REQ[t].compare_exchange(0, now_ns(), Ordering::AcqRel, Ordering::Relaxed).is_ok() {
        SAMPLE_WHY[t].store(why, Ordering::Relaxed);
        SAMPLE_FROM[t].store(cpu(), Ordering::Relaxed);
        if let Some(apic_id) = crate::arch_impl::x86_64::smp::cpu_apic_id(t) {
            let _ = crate::arch_impl::x86_64::apic::send_ipi(apic_id, crate::arch_impl::x86_64::apic::Ipi::Nmi);
        }
    }
}

/// From the NMI handler: answer a pending sample request for this CPU.
pub fn nmi_sample(c: usize, rip: u64, rsp: u64) {
    if c >= 8 {
        return;
    }
    let req = SAMPLE_REQ[c].load(Ordering::Acquire);
    if req == 0 {
        return;
    }
    let now = now_ns();
    let why = SAMPLE_WHY[c].load(Ordering::Relaxed);
    let held = if why == 1 {
        now.wrapping_sub(SCHED_ACQ.load(Ordering::Relaxed))
    } else {
        now.wrapping_sub(PM_ACQ_NS.load(Ordering::Relaxed))
    };
    push(NMI_SAMPLE, [now - req, req, c as u64, rip, why, held, SAMPLE_FROM[c].load(Ordering::Relaxed), 0, 0]);
    // Heuristic callers: kernel-text words on the interrupted stack.
    let anchor = nmi_sample as usize as u64;
    let (lo, hi) = (anchor.saturating_sub(0x80_0000), anchor + 0x80_0000);
    let mut found = [0u64; 8];
    let mut n = 0;
    if rsp >= 0x8000_0000_0000 && rsp % 8 == 0 {
        let words = (0x1000 - (rsp & 0xfff)) / 8;
        for i in 0..words {
            let p = rsp + i * 8;
            let w = unsafe { core::ptr::read_volatile(p as *const u64) };
            if w >= lo && w < hi {
                found[n] = w;
                n += 1;
                if n == found.len() {
                    break;
                }
            }
        }
    }
    push(NMI_STACK, [c as u64, req, found[0], found[1], found[2], found[3], found[4], found[5], found[6]]);
    SAMPLE_REQ[c].store(0, Ordering::Release);
}
pub static IPI_NS: [AtomicU64; 8] = [const { AtomicU64::new(0) }; 8];
static PM_IPI0: AtomicU64 = AtomicU64::new(0);
pub fn ipi_time(t0: u64, kind: u64, dest: u64) {
    let dt = now_ns().wrapping_sub(t0);
    let c = cpu() as usize;
    if c < 8 {
        IPI_NS[c].fetch_add(dt, Ordering::Relaxed);
    }
    if dt > 1_000_000 {
        push(IPI_SLOW, [dt, t0, c as u64, kind, dest, 0, 0, 0, 0]);
    }
}
fn ipi_here() -> u64 {
    let c = cpu() as usize;
    if c < 8 { IPI_NS[c].load(Ordering::Relaxed) } else { 0 }
}
pub static STAGE: [[AtomicU64; 4]; 8] = [const { [const { AtomicU64::new(0) }; 4] }; 8];
pub fn stage(i: usize) {
    let c = cpu() as usize;
    if c < 8 {
        STAGE[c][i].store(now_ns(), Ordering::Relaxed);
    }
}
pub static POLL_FAILS: AtomicU64 = AtomicU64::new(0);

struct WaitSlot {
    tid: AtomicU64,
    start: AtomicU64,
    wake: AtomicU64,
    resume: AtomicU64,
    exit: AtomicU64,
    out: AtomicU64,
    out_cpu: AtomicU64,
}
static WAITS: [WaitSlot; 64] = [const {
    WaitSlot {
        tid: AtomicU64::new(0),
        start: AtomicU64::new(0),
        wake: AtomicU64::new(0),
        resume: AtomicU64::new(0),
        exit: AtomicU64::new(0),
        out: AtomicU64::new(0),
        out_cpu: AtomicU64::new(0),
    }
}; 64];

pub fn wait_begin(tid: u64, wake_ns: u64) {
    let w = &WAITS[(tid % 64) as usize];
    w.tid.store(tid, Ordering::Relaxed);
    w.start.store(now_ns(), Ordering::Relaxed);
    w.wake.store(wake_ns, Ordering::Relaxed);
    w.resume.store(0, Ordering::Relaxed);
    w.exit.store(0, Ordering::Relaxed);
    w.out.store(0, Ordering::Relaxed);
    POPS[(tid % 64) as usize][0].store(0, Ordering::Relaxed);
}

pub fn wait_resume(tid: u64) {
    let w = &WAITS[(tid % 64) as usize];
    if w.tid.load(Ordering::Relaxed) == tid && w.resume.load(Ordering::Relaxed) == 0 {
        w.resume.store(now_ns(), Ordering::Relaxed);
    }
}

pub fn wait_end(tid: u64) {
    let w = &WAITS[(tid % 64) as usize];
    if w.tid.load(Ordering::Relaxed) != tid {
        return;
    }
    let now = now_ns();
    w.exit.store(now, Ordering::Relaxed);
    let wake = w.wake.load(Ordering::Relaxed);
    let (pop_ns, pop_cpu, arm, target) = pop_of(tid);
    let tcur = POP_TCUR[(tid % 64) as usize].load(Ordering::Relaxed);
    push(
        TIMED,
        [
            tid,
            wake,
            now.wrapping_sub(wake),
            if pop_ns == 0 { u64::MAX } else { pop_ns.wrapping_sub(wake) },
            pop_cpu | (arm << 8) | (target << 16),
            tcur,
            w.resume.load(Ordering::Relaxed).wrapping_sub(wake),
            cpu(),
            w.start.load(Ordering::Relaxed),
        ],
    );
}

/// A switch on this CPU from `old` to `new`.
pub fn note_switch(old: u64, new: u64) {
    let now = now_ns();
    let w = &WAITS[(old % 64) as usize];
    if w.tid.load(Ordering::Relaxed) == old {
        let exit = w.exit.load(Ordering::Relaxed);
        if exit != 0 && w.out.load(Ordering::Relaxed) == 0 && now.wrapping_sub(exit) < 5_000_000 {
            w.out.store(now, Ordering::Relaxed);
            w.out_cpu.store(cpu(), Ordering::Relaxed);
        }
    }
    let w = &WAITS[(new % 64) as usize];
    if w.tid.load(Ordering::Relaxed) == new {
        let out = w.out.load(Ordering::Relaxed);
        if out != 0 {
            let exit = w.exit.load(Ordering::Relaxed);
            push(POST_EXIT, [new, exit, out - exit, now - out, w.out_cpu.load(Ordering::Relaxed), cpu(), 0, 0, 0]);
            w.out.store(0, Ordering::Relaxed);
            w.exit.store(0, Ordering::Relaxed);
        }
    }
}

static POP_TCUR: [AtomicU64; 64] = [const { AtomicU64::new(0) }; 64];
pub fn note_pop_tcur(tid: u64, tcur: u64) {
    POP_TCUR[(tid % 64) as usize].store(tcur, Ordering::Relaxed);
}
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
    log::info!(
        "[P1401 SCHED_SUMMARY at_ms={} n={} total_us={} max_us={} gt1ms={} threads={} poll_fails={} anchor={:#x}]",
        now_ns() / 1_000_000,
        BODY_N.swap(0, Ordering::Relaxed),
        BODY_TOTAL.swap(0, Ordering::Relaxed) / 1000,
        BODY_MAX.swap(0, Ordering::Relaxed) / 1000,
        BODY_GT1.swap(0, Ordering::Relaxed),
        THREADS_LEN.load(Ordering::Relaxed),
        POLL_FAILS.swap(0, Ordering::Relaxed),
        nmi_sample as usize
    );
    for i in 0..32 {
        let n = PF_N[i].swap(0, Ordering::Relaxed);
        if n > 1000 {
            log::info!(
                "[P1401 PF tid={} n={} cr2={:#x} rip={:#x} err={:#x}]",
                PF_TID[i].load(Ordering::Relaxed),
                n,
                PF_CR2[i].load(Ordering::Relaxed),
                PF_RIP[i].load(Ordering::Relaxed),
                PF_ERR[i].load(Ordering::Relaxed)
            );
        }
        PF_TID[i].store(0, Ordering::Relaxed);
    }
    for i in 0..16 {
        let l = OCC_LINE[i].load(Ordering::Relaxed);
        let n = OCC_N[i].swap(0, Ordering::Relaxed);
        let ns = OCC_NS[i].swap(0, Ordering::Relaxed);
        if l != 0 && ns > 5_000_000 {
            log::info!("[P1401 PM_OCC line={} n={} total_us={}]", l, n, ns / 1000);
        }
    }
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
                "[P1401 IDLE_GAP us={} at_ms={} cpu={} switches={} ticks={} steps={}]",
                w[1] / 1000,
                w[2] / 1_000_000,
                w[3],
                w[4],
                w[5],
                w[6]
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
            HOUSEKEEP => log::info!(
                "[P1401 HOUSEKEEP step={} us={} at_ms={} cpu={} ticks={}]",
                w[1],
                w[2] / 1000,
                w[3] / 1_000_000,
                w[4],
                w[5]
            ),
            TIMED => log::info!(
                "[P1401 TIMED tid={} start_ms={} wake_us={} exit_rel_us={} pop_rel_us={} pop_cpu={} arm={} target={} tcur={} resume_rel_us={} exit_cpu={}]",
                w[1],
                w[9] / 1_000_000,
                w[2] / 1000,
                w[3] as i64 / 1000,
                w[4] as i64 / 1000,
                w[5] & 0xff,
                (w[5] >> 8) & 0xff,
                w[5] >> 16,
                w[6],
                w[7] as i64 / 1000,
                w[8]
            ),
            POST_EXIT => log::info!(
                "[P1401 POST_EXIT tid={} exit_ms={} out_after_us={} back_after_us={} out_cpu={} in_cpu={}]",
                w[1],
                w[2] / 1_000_000,
                w[3] / 1000,
                w[4] / 1000,
                w[5],
                w[6]
            ),
            STAGES => {
                let f = |v: u64| if v == u64::MAX { -1 } else { (v / 1000) as i64 };
                log::info!(
                    "[P1401 STAGES held_us={} at_ms={} cpu={} line={} s0_us={} s1_us={} s2_us={} s3_us={} ipi_us={}]",
                    w[1] / 1000,
                    w[2] / 1_000_000,
                    w[3],
                    w[8],
                    f(w[4]),
                    f(w[5]),
                    f(w[6]),
                    f(w[7]),
                    w[9] / 1000
                )
            }
            IPI_SLOW => log::info!(
                "[P1401 IPI_SLOW us={} at_ms={} cpu={} kind={} dest={}]",
                w[1] / 1000,
                w[2] / 1_000_000,
                w[3],
                w[4],
                w[5]
            ),
            NMI_SAMPLE => log::info!(
                "[P1401 NMI_SAMPLE lat_us={} at_ms={} cpu={} rip={:#x} why={} held_us={} from={}]",
                w[1] / 1000,
                w[2] / 1_000_000,
                w[3],
                w[4],
                w[5],
                w[6] / 1000,
                w[7]
            ),
            NMI_STACK => log::info!(
                "[P1401 NMI_STACK cpu={} req_ms={} {:#x} {:#x} {:#x} {:#x} {:#x} {:#x} {:#x}]",
                w[1],
                w[2] / 1_000_000,
                w[3],
                w[4],
                w[5],
                w[6],
                w[7],
                w[8],
                w[9]
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

pub fn pm_acq_ns() -> u64 {
    PM_ACQ_NS.load(Ordering::Relaxed)
}

pub fn pm_acquired(loc: &'static core::panic::Location<'static>) {
    PM_ACQ_NS.store(now_ns(), Ordering::Relaxed);
    PM_FILE.store(loc.file().as_ptr() as u64, Ordering::Relaxed);
    PM_FILE_LEN.store(loc.file().len() as u64, Ordering::Relaxed);
    PM_LINE.store(loc.line() as u64, Ordering::Relaxed);
    PM_SHOOT0.store(shootdowns_here(), Ordering::Relaxed);
    PM_IPI0.store(ipi_here(), Ordering::Relaxed);
    let c = cpu() as usize;
    if c < 8 {
        for st in STAGE[c].iter() {
            st.store(0, Ordering::Relaxed);
        }
    }
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
    if start != 0 {
        occ(PM_LINE.load(Ordering::Relaxed), held);
    }
    if start != 0 && held > 5_000_000 {
        let c = cpu() as usize;
        if c < 8 {
            let rel = |i: usize| {
                let v = STAGE[c][i].load(Ordering::Relaxed);
                if v == 0 { u64::MAX } else { v.wrapping_sub(start) }
            };
            push(STAGES, [held, start, c as u64, rel(0), rel(1), rel(2), rel(3), PM_LINE.load(Ordering::Relaxed), ipi_here().wrapping_sub(PM_IPI0.load(Ordering::Relaxed))]);
        }
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
static IDLE_TICKS: [AtomicU64; 8] = [const { AtomicU64::new(0) }; 8];
pub static STEPS: [AtomicU64; 8] = [const { AtomicU64::new(0) }; 8];
static IDLE_STEPS: [AtomicU64; 8] = [const { AtomicU64::new(0) }; 8];

fn ticks_on(cpu: usize) -> u64 {
    crate::tracing::providers::counters::TIMER_TICK_TOTAL.get_cpu(cpu)
}

/// Time one idle-housekeeping step on this CPU.
pub fn step<R>(id: u64, f: impl FnOnce() -> R) -> R {
    let c = cpu() as usize;
    let t0 = now_ns();
    let k0 = if c < 8 { ticks_on(c) } else { 0 };
    let r = f();
    if c < 8 {
        STEPS[c].fetch_add(1, Ordering::Relaxed);
    }
    let dt = now_ns() - t0;
    if dt > 10_000_000 {
        let k1 = if c < 8 { ticks_on(c) } else { 0 };
        push(HOUSEKEEP, [id, dt, t0, c as u64, k1 - k0, 0, 0, 0, 0]);
    }
    r
}
pub static SWITCHES: [AtomicU64; 8] = [const { AtomicU64::new(0) }; 8];

pub fn idle_pass(cpu: usize) {
    if cpu >= 8 {
        return;
    }
    let now = now_ns();
    let last = IDLE_LAST[cpu].swap(now, Ordering::Relaxed);
    let sw = SWITCHES[cpu].load(Ordering::Relaxed);
    let last_sw = IDLE_SWITCHES[cpu].swap(sw, Ordering::Relaxed);
    let tk = ticks_on(cpu);
    let last_tk = IDLE_TICKS[cpu].swap(tk, Ordering::Relaxed);
    let st = STEPS[cpu].load(Ordering::Relaxed);
    let last_st = IDLE_STEPS[cpu].swap(st, Ordering::Relaxed);
    if last != 0 && now - last > 30_000_000 && sw == last_sw {
        push(IDLE_GAP, [now - last, last, cpu as u64, sw, tk - last_tk, st - last_st, 0, 0, 0]);
    }
}
