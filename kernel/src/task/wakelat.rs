//! SCRATCH (never merged): wake-to-run latency tracer for #1357/#1350/#1352.
//! Lock-free stamps; read and reset through /proc/trace/counters.

use core::sync::atomic::{AtomicU64, Ordering::Relaxed};

const SLOTS: usize = 4096;
const CPUS: usize = 8;
pub const PATHS: usize = 12;
pub const P_IO_CURRENT: usize = 0;
pub const P_IO_QUEUED: usize = 1;
pub const P_IO_HELD: usize = 2;
pub const P_UNB_CURRENT: usize = 3;
pub const P_UNB_QUEUED_IDLE: usize = 4;
pub const P_UNB_QUEUED_BUSY: usize = 5;
pub const P_UNB_HELD: usize = 6;
pub const P_TIMER_CURRENT: usize = 7;
pub const P_TIMER_QUEUED: usize = 8;
pub const P_SIG: usize = 9;
const NAMES: [&str; PATHS] = [
    "io-current", "io-queued", "io-held", "unb-current", "unb-queued-idle", "unb-queued-busy",
    "unb-held", "timer-current", "timer-queued", "sig", "-", "-",
];
const BUCKETS: [u64; 11] = [10, 25, 50, 100, 250, 500, 1000, 2000, 5000, 10000, 50000];
const NB: usize = BUCKETS.len() + 1;

static R_TID: [AtomicU64; SLOTS] = [const { AtomicU64::new(0) }; SLOTS];
static R_TS: [AtomicU64; SLOTS] = [const { AtomicU64::new(0) }; SLOTS];
/// path | target<<8 | waker<<16 | target_halted<<24 | target_idle<<25 | reclaim_on_target<<26
/// | target_current_tid<<32
static R_INFO: [AtomicU64; SLOTS] = [const { AtomicU64::new(0) }; SLOTS];

static IPI_TS: [AtomicU64; CPUS] = [const { AtomicU64::new(0) }; CPUS];
static IRQ_TS: [AtomicU64; CPUS] = [const { AtomicU64::new(0) }; CPUS];
static HALTED: [AtomicU64; CPUS] = [const { AtomicU64::new(0) }; CPUS];
static RECLAIM: [AtomicU64; CPUS] = [const { AtomicU64::new(0) }; CPUS];
static WAKE_ENTRY: [AtomicU64; CPUS] = [const { AtomicU64::new(0) }; CPUS];

static COUNT: [AtomicU64; PATHS] = [const { AtomicU64::new(0) }; PATHS];
static SUM: [AtomicU64; PATHS] = [const { AtomicU64::new(0) }; PATHS];
static MAX: [AtomicU64; PATHS] = [const { AtomicU64::new(0) }; PATHS];
static S_WAKE: [AtomicU64; PATHS] = [const { AtomicU64::new(0) }; PATHS];
static S_IPI: [AtomicU64; PATHS] = [const { AtomicU64::new(0) }; PATHS];
static S_DELIV: [AtomicU64; PATHS] = [const { AtomicU64::new(0) }; PATHS];
static S_DISP: [AtomicU64; PATHS] = [const { AtomicU64::new(0) }; PATHS];
static N_SEG: [AtomicU64; PATHS] = [const { AtomicU64::new(0) }; PATHS];
static N_HALTED: [AtomicU64; PATHS] = [const { AtomicU64::new(0) }; PATHS];
static HIST: [[AtomicU64; NB]; PATHS] = [const { [const { AtomicU64::new(0) }; NB] }; PATHS];

const OUT: usize = 48;
static OUT_N: AtomicU64 = AtomicU64::new(0);
static OUT_A: [AtomicU64; OUT] = [const { AtomicU64::new(0) }; OUT];
static OUT_B: [AtomicU64; OUT] = [const { AtomicU64::new(0) }; OUT];
static OUT_C: [AtomicU64; OUT] = [const { AtomicU64::new(0) }; OUT];

static TL_N: AtomicU64 = AtomicU64::new(0);
static TL_SUM: AtomicU64 = AtomicU64::new(0);
static TL_MAX: AtomicU64 = AtomicU64::new(0);
static TL_HIST: [AtomicU64; NB] = [const { AtomicU64::new(0) }; NB];
static PX_N: AtomicU64 = AtomicU64::new(0);
static PX_MAX: AtomicU64 = AtomicU64::new(0);
static PX_HIST: [AtomicU64; NB] = [const { AtomicU64::new(0) }; NB];
static PASS_LAST: AtomicU64 = AtomicU64::new(0);
static PASS_GAP_MAX: AtomicU64 = AtomicU64::new(0);
static PASS_GAP_HIST: [AtomicU64; NB] = [const { AtomicU64::new(0) }; NB];
static RC_N: AtomicU64 = AtomicU64::new(0);
static RC_MAX: AtomicU64 = AtomicU64::new(0);
static RC_HIST: [AtomicU64; NB] = [const { AtomicU64::new(0) }; NB];
static SD_N: AtomicU64 = AtomicU64::new(0);
static SD_MAX: AtomicU64 = AtomicU64::new(0);
static SD_HIST: [AtomicU64; NB] = [const { AtomicU64::new(0) }; NB];

#[inline(always)]
pub fn tsc() -> u64 {
    crate::arch_read_timestamp()
}

fn us(cycles: u64) -> u64 {
    crate::arch_ticks_to_nanos(cycles) / 1000
}

fn bucket(v_us: u64) -> usize {
    BUCKETS.iter().position(|&b| v_us < b).unwrap_or(BUCKETS.len())
}

fn cpu() -> usize {
    crate::task::scheduler::current_cpu().min(CPUS - 1)
}

fn max(a: &AtomicU64, v: u64) {
    a.fetch_max(v, Relaxed);
}

/// A thread became Ready: `target` is the CPU expected to run it.
pub fn ready(tid: u64, path: usize, target: usize, target_idle: bool, target_current: u64) {
    let i = tid as usize % SLOTS;
    let t = target.min(CPUS - 1);
    let info = path as u64
        | (t as u64) << 8
        | (cpu() as u64) << 16
        | ((HALTED[t].load(Relaxed) != 0) as u64) << 24
        | (target_idle as u64) << 25
        | ((RECLAIM[t].load(Relaxed) != 0) as u64) << 26
        | (target_current & 0xffff_ffff) << 32;
    R_INFO[i].store(info, Relaxed);
    R_TID[i].store(tid, Relaxed);
    R_TS[i].store(tsc(), Relaxed);
    ev_on(t, E_READY, tid << 8 | path as u64);
}

/// Thread `tid` runs again on this CPU.
pub fn ran(tid: u64, site: u64) {
    let i = tid as usize % SLOTS;
    if R_TID[i].load(Relaxed) != tid {
        return;
    }
    let start = R_TS[i].swap(0, Relaxed);
    if start == 0 {
        return;
    }
    let now = tsc();
    let info = R_INFO[i].load(Relaxed);
    let path = (info & 0xff) as usize % PATHS;
    let target = ((info >> 8) & 0xff) as usize;
    let waker = ((info >> 16) & 0xff) as usize % CPUS;
    let me = cpu();
    let total = us(now.saturating_sub(start));
    COUNT[path].fetch_add(1, Relaxed);
    SUM[path].fetch_add(total, Relaxed);
    max(&MAX[path], total);
    HIST[path][bucket(total)].fetch_add(1, Relaxed);
    if info >> 24 & 1 != 0 {
        N_HALTED[path].fetch_add(1, Relaxed);
    }
    let wake_entry = WAKE_ENTRY[waker].load(Relaxed);
    let ipi = IPI_TS[target.min(CPUS - 1)].load(Relaxed);
    let irq = IRQ_TS[me].load(Relaxed);
    let (mut sw, mut si, mut sd, mut sp) = (0, 0, 0, 0);
    if wake_entry != 0 && wake_entry <= start && start - wake_entry < start / 2 {
        sw = us(start - wake_entry);
        S_WAKE[path].fetch_add(sw, Relaxed);
    }
    if ipi >= start && ipi <= now && irq >= ipi && irq <= now {
        si = us(ipi - start);
        sd = us(irq - ipi);
        sp = us(now - irq);
        S_IPI[path].fetch_add(si, Relaxed);
        S_DELIV[path].fetch_add(sd, Relaxed);
        S_DISP[path].fetch_add(sp, Relaxed);
        N_SEG[path].fetch_add(1, Relaxed);
    }
    if total >= 1200 && total < 20000 {
        snapshot(me, start, tid << 24 | total.min(0xff_ffff));
    }
    if total >= 2000 {
        let k = OUT_N.fetch_add(1, Relaxed) as usize;
        if k < OUT {
            OUT_A[k].store(tid << 32 | total.min(0xffff_ffff), Relaxed);
            OUT_B[k].store(info, Relaxed);
            OUT_C[k].store(
                si.min(0xffff) | sd.min(0xffff) << 16 | sp.min(0xffff) << 32 | (me as u64) << 48 | site << 56,
                Relaxed,
            );
        }
    }
    let _ = sw;
}

pub fn ipi_sent(target: usize) {
    if target < CPUS {
        IPI_TS[target].store(tsc(), Relaxed);
        ev_on(target, E_IPI_IN, (cpu() as u64) << 8);
    }
}

pub fn irq_entry(cpu: usize, vector: u64) {
    if cpu < CPUS {
        IRQ_TS[cpu].store(tsc(), Relaxed);
        ev_on(cpu, E_IRQ, vector);
    }
}

pub fn halt(on: bool) {
    HALTED[cpu()].store(on as u64, Relaxed);
    ev(if on { E_HALT_IN } else { E_HALT_OUT }, 0);
}

pub fn wake_entry() {
    WAKE_ENTRY[cpu()].store(tsc(), Relaxed);
}

pub fn reclaim(on: bool) -> u64 {
    RECLAIM[cpu()].store(on as u64, Relaxed);
    ev(if on { E_RECLAIM_IN } else { E_RECLAIM_OUT }, 0);
    tsc()
}

pub fn reclaim_done(start: u64) {
    let d = us(tsc().saturating_sub(start));
    RC_N.fetch_add(1, Relaxed);
    max(&RC_MAX, d);
    RC_HIST[bucket(d)].fetch_add(1, Relaxed);
}

pub fn shootdown_done(start: u64) {
    let d = us(tsc().saturating_sub(start));
    SD_N.fetch_add(1, Relaxed);
    max(&SD_MAX, d);
    SD_HIST[bucket(d)].fetch_add(1, Relaxed);
}

/// A live timer-heap entry popped `late_ns` after its deadline.
pub fn timer_late(late_ns: u64) {
    let d = late_ns / 1000;
    TL_N.fetch_add(1, Relaxed);
    TL_SUM.fetch_add(d, Relaxed);
    max(&TL_MAX, d);
    TL_HIST[bucket(d)].fetch_add(1, Relaxed);
}

pub fn posix_late(late_ns: u64) {
    let d = late_ns / 1000;
    PX_N.fetch_add(1, Relaxed);
    max(&PX_MAX, d);
    PX_HIST[bucket(d)].fetch_add(1, Relaxed);
}

/// A timer pass ran (under the scheduler lock).
pub fn timer_pass() {
    let now = tsc();
    let last = PASS_LAST.swap(now, Relaxed);
    if last != 0 && now > last {
        let d = us(now - last);
        max(&PASS_GAP_MAX, d);
        PASS_GAP_HIST[bucket(d)].fetch_add(1, Relaxed);
    }
}

fn hist(out: &mut alloc::string::String, h: &[AtomicU64; NB]) {
    use core::fmt::Write;
    for b in h.iter() {
        let _ = write!(out, " {}", b.swap(0, Relaxed));
    }
}

/// Report and reset.
pub fn report(out: &mut alloc::string::String) {
    use core::fmt::Write;
    let _ = writeln!(out, "WAKELAT buckets_us <10 <25 <50 <100 <250 <500 <1000 <2000 <5000 <10000 <50000 >=50000");
    for p in 0..PATHS {
        let n = COUNT[p].swap(0, Relaxed);
        let sum = SUM[p].swap(0, Relaxed);
        let mx = MAX[p].swap(0, Relaxed);
        let nseg = N_SEG[p].swap(0, Relaxed).max(1);
        let (w, i, d, s) = (
            S_WAKE[p].swap(0, Relaxed),
            S_IPI[p].swap(0, Relaxed),
            S_DELIV[p].swap(0, Relaxed),
            S_DISP[p].swap(0, Relaxed),
        );
        let halted = N_HALTED[p].swap(0, Relaxed);
        if n == 0 {
            for b in HIST[p].iter() { b.store(0, Relaxed); }
            continue;
        }
        let _ = write!(
            out,
            "WAKELAT path={} n={} avg_us={} max_us={} halted={} wake_avg={} seg_n={} ipi_avg={} deliver_avg={} dispatch_avg={} hist",
            NAMES[p], n, sum / n, mx, halted, w / n, nseg, i / nseg, d / nseg, s / nseg
        );
        hist(out, &HIST[p]);
        let _ = writeln!(out);
    }
    let n = OUT_N.swap(0, Relaxed).min(OUT as u64) as usize;
    for k in 0..n {
        let a = OUT_A[k].load(Relaxed);
        let b = OUT_B[k].load(Relaxed);
        let c = OUT_C[k].load(Relaxed);
        let _ = writeln!(
            out,
            "WAKELAT slow tid={} us={} path={} target={} waker={} halted={} idle={} reclaim={} target_cur={} ipi_us={} deliver_us={} dispatch_us={} ran_cpu={} site={}",
            a >> 32, a & 0xffff_ffff, NAMES[(b & 0xff) as usize % PATHS], (b >> 8) & 0xff, (b >> 16) & 0xff,
            (b >> 24) & 1, (b >> 25) & 1, (b >> 26) & 1, b >> 32,
            c & 0xffff, (c >> 16) & 0xffff, (c >> 32) & 0xffff, (c >> 48) & 0xff, c >> 56
        );
    }
    report_snapshots(out);
    let tn = TL_N.swap(0, Relaxed).max(1);
    let _ = write!(out, "WAKELAT timer-late n={} avg_us={} max_us={} hist", tn, TL_SUM.swap(0, Relaxed) / tn, TL_MAX.swap(0, Relaxed));
    hist(out, &TL_HIST);
    let _ = write!(out, "\nWAKELAT posix-late n={} max_us={} hist", PX_N.swap(0, Relaxed), PX_MAX.swap(0, Relaxed));
    hist(out, &PX_HIST);
    let _ = write!(out, "\nWAKELAT pass-gap max_us={} hist", PASS_GAP_MAX.swap(0, Relaxed));
    hist(out, &PASS_GAP_HIST);
    let _ = write!(out, "\nWAKELAT reclaim n={} max_us={} hist", RC_N.swap(0, Relaxed), RC_MAX.swap(0, Relaxed));
    hist(out, &RC_HIST);
    let _ = write!(out, "\nWAKELAT shootdown n={} max_us={} hist", SD_N.swap(0, Relaxed), SD_MAX.swap(0, Relaxed));
    hist(out, &SD_HIST);
    let _ = writeln!(out);
}

// ---------------------------------------------------------------------------
// Per-CPU event ring, snapshotted for slow wakes.
pub const E_IRQ: u64 = 1; // arg: in-service vector
pub const E_HALT_IN: u64 = 2; // arg: tid (0 idle)
pub const E_HALT_OUT: u64 = 3;
pub const E_DISPATCH: u64 = 4; // arg: tid
pub const E_IPI_IN: u64 = 5; // arg: sender cpu << 8 | vector
pub const E_READY: u64 = 6; // arg: tid << 8 | path
pub const E_RECLAIM_IN: u64 = 7;
pub const E_RECLAIM_OUT: u64 = 8;
pub const E_LOG_IN: u64 = 9; // arg: COM port
pub const E_LOG_OUT: u64 = 10;
pub const E_LEAVE_SET: u64 = 11; // arg: tid
pub const E_LEAVE_CLR: u64 = 12; // arg: tid
pub const E_PASSOVER: u64 = 13; // arg: tid << 8 | owner cpu
pub const E_IDLE_PASS: u64 = 14;
pub const E_SWITCH: u64 = 15; // arg: old << 32 | new
const RING: usize = 128;
static RING_POS: [AtomicU64; CPUS] = [const { AtomicU64::new(0) }; CPUS];
static RING_TS: [[AtomicU64; RING]; CPUS] = [const { [const { AtomicU64::new(0) }; RING] }; CPUS];
static RING_EV: [[AtomicU64; RING]; CPUS] = [const { [const { AtomicU64::new(0) }; RING] }; CPUS];

pub fn ev_on(cpu: usize, kind: u64, arg: u64) {
    if cpu >= CPUS {
        return;
    }
    let k = RING_POS[cpu].fetch_add(1, Relaxed) as usize % RING;
    RING_TS[cpu][k].store(tsc(), Relaxed);
    RING_EV[cpu][k].store(kind | arg << 8, Relaxed);
}

pub fn ev(kind: u64, arg: u64) {
    ev_on(cpu(), kind, arg);
}

const SNAPS: usize = 6;
const SNAP_LEN: usize = 48;
static SNAP_N: AtomicU64 = AtomicU64::new(0);
static SNAP_HDR: [AtomicU64; SNAPS] = [const { AtomicU64::new(0) }; SNAPS];
static SNAP_START: [AtomicU64; SNAPS] = [const { AtomicU64::new(0) }; SNAPS];
static SNAP_TS: [[AtomicU64; SNAP_LEN]; SNAPS] = [const { [const { AtomicU64::new(0) }; SNAP_LEN] }; SNAPS];
static SNAP_EV: [[AtomicU64; SNAP_LEN]; SNAPS] = [const { [const { AtomicU64::new(0) }; SNAP_LEN] }; SNAPS];

/// Copy `cpu`'s events in [start, now] (the last SNAP_LEN) into a snapshot.
pub fn snapshot(cpu: usize, start: u64, hdr: u64) {
    if cpu >= CPUS {
        return;
    }
    let s = SNAP_N.fetch_add(1, Relaxed) as usize;
    if s >= SNAPS {
        return;
    }
    let pos = RING_POS[cpu].load(Relaxed) as usize;
    let mut picked = [(0u64, 0u64); SNAP_LEN];
    let mut n = 0;
    for back in 1..=RING.min(pos) {
        let k = (pos - back) % RING;
        let ts = RING_TS[cpu][k].load(Relaxed);
        if ts < start && us(start - ts) > 1000 {
            break;
        }
        if n < SNAP_LEN {
            picked[n] = (ts, RING_EV[cpu][k].load(Relaxed));
            n += 1;
        }
    }
    SNAP_HDR[s].store(hdr | (cpu as u64) << 56, Relaxed);
    SNAP_START[s].store(start, Relaxed);
    for i in 0..SNAP_LEN {
        let (ts, e) = if i < n { picked[n - 1 - i] } else { (0, 0) };
        SNAP_TS[s][i].store(ts, Relaxed);
        SNAP_EV[s][i].store(e, Relaxed);
    }
}

pub fn report_snapshots(out: &mut alloc::string::String) {
    use core::fmt::Write;
    let n = SNAP_N.swap(0, Relaxed).min(SNAPS as u64) as usize;
    for s in 0..n {
        let hdr = SNAP_HDR[s].load(Relaxed);
        let start = SNAP_START[s].load(Relaxed);
        let _ = write!(out, "WAKELAT snap{} cpu={} tid={} us={} ev:", s, hdr >> 56, (hdr >> 24) & 0xffff_ffff, hdr & 0xff_ffff);
        for i in 0..SNAP_LEN {
            let ts = SNAP_TS[s][i].load(Relaxed);
            if ts == 0 {
                continue;
            }
            let e = SNAP_EV[s][i].load(Relaxed);
            let rel = if ts >= start { us(ts - start) as i64 } else { -(us(start - ts) as i64) };
            let _ = write!(out, " {}:{}/{:x}", rel, e & 0xff, e >> 8);
        }
        let _ = writeln!(out);
    }
}
