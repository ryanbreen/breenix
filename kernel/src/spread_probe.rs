//! SCRATCH (#1540): force one CPU to stall inside the guest and record how
//! far the stall spreads to the other CPUs. Never lands.
//!
//! Every 500 ms one forcing is armed, rotating through three kinds:
//! - UNAVAIL: an idle CPU about to halt sends itself an NMI whose handler
//!   spins 100 ms, so for that long it answers neither interrupts nor NMIs,
//!   like a vCPU the host is not running.
//! - IPI: the next reschedule IPI send spins 30 ms with interrupts masked
//!   before the APIC write, like the slow APIC writes measured on the gate.
//! - DRAIN: the next 16 COM2 port writes an idle CPU's log drain makes each
//!   spin 5 ms, with interrupts as the drain has them.
//!
//! For each window it records, per CPU, the longest stretch without a
//! timer-entry interrupt, the longest scheduler-lock and process-manager
//! waits, and the longest TLB shootdown wait, all overlapping the window,
//! and kstrandd prints one SPREAD line per window.

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};

use crate::arch_impl::x86_64::timer::{frequency_hz, rdtsc};
use crate::task::scheduler::MAX_CPUS;

pub const UNAVAIL: u32 = 1;
pub const IPI: u32 = 2;
pub const DRAIN: u32 = 3;
/// A CPU that has just taken PROCESS_MANAGER with interrupts masked becomes
/// unavailable for 50 ms while holding it.
pub const PMHOLD: u32 = 4;

const ST_IDLE: u32 = 0;
const ST_ARMED: u32 = 1;
const ST_STARTING: u32 = 5;
const ST_OPEN: u32 = 2;
const ST_CLOSED: u32 = 3;
const ST_DONE: u32 = 4;

static STATE: AtomicU32 = AtomicU32::new(ST_IDLE);
static ENABLED: AtomicBool = AtomicBool::new(false);
static KIND: AtomicU32 = AtomicU32::new(0);
static ROTATE: AtomicU32 = AtomicU32::new(0);
static NEXT_ARM: AtomicU64 = AtomicU64::new(0);
static WCPU: AtomicU64 = AtomicU64::new(0);
static WSTART: AtomicU64 = AtomicU64::new(0);
static WEND: AtomicU64 = AtomicU64::new(u64::MAX);
static WSEQ: AtomicU64 = AtomicU64::new(0);
/// bit 0: the forcing CPU held the scheduler lock; bit 1: it held the PM.
static WFLAG: AtomicU64 = AtomicU64::new(0);
static DRAIN_LEFT: AtomicU32 = AtomicU32::new(0);

static SAMPLE_WANTED: [AtomicBool; MAX_CPUS] = [const { AtomicBool::new(false) }; MAX_CPUS];
static SAMPLE_RIP: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];
static SAMPLE_FLAGS: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];
static FORCE_NMI: [AtomicBool; MAX_CPUS] = [const { AtomicBool::new(false) }; MAX_CPUS];
static IN_IDLE_DRAIN: [AtomicBool; MAX_CPUS] = [const { AtomicBool::new(false) }; MAX_CPUS];

static LAST_INT: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];
static GAP: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];
static SCHED_WAIT: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];
static PM_WAIT: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];
static SD_WAIT: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];
/// Longest stretch from a dispatch that found PROCESS_MANAGER busy to the
/// next one that got it.
static DISPATCH_BLOCKED: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];
static DISPATCH_FAIL_SINCE: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];
static SD_MAX: AtomicU64 = AtomicU64::new(0);
static SD_MAX_LOC: AtomicUsize = AtomicUsize::new(0);
static SD_MAX_TGT: AtomicU64 = AtomicU64::new(0);
static SD_MAX_HELD: AtomicU64 = AtomicU64::new(0);

pub static SCHED_HOLDER: AtomicU64 = AtomicU64::new(u64::MAX);

/// This build forces nothing; it records late timed-futex returns.
const FORCING: bool = false;

fn mono_ns() -> u64 {
    let (s, n) = crate::time::get_monotonic_time_ns();
    s as u64 * 1_000_000_000 + n as u64
}

const TIDS: usize = 512;
/// Per tid slot: the deadline the timer heap popped, when, and on which CPU.
static POP_DEADLINE: [AtomicU64; TIDS] = [const { AtomicU64::new(0) }; TIDS];
static POP_AT: [AtomicU64; TIDS] = [const { AtomicU64::new(0) }; TIDS];
static POP_CPU: [AtomicU64; TIDS] = [const { AtomicU64::new(0) }; TIDS];
/// Where the timer wake placed it: target CPU, +100 when that CPU was idle,
/// +1000 when no CPU was chosen; u64::MAX when it was still current somewhere.
static PLACED: [AtomicU64; TIDS] = [const { AtomicU64::new(0) }; TIDS];
static DISPATCH_AT: [AtomicU64; TIDS] = [const { AtomicU64::new(0) }; TIDS];
static DISPATCH_CPU: [AtomicU64; TIDS] = [const { AtomicU64::new(0) }; TIDS];

/// Per CPU, the last 16 stretches over 10 ms without a timer-entry interrupt:
/// (monotonic ns at the interrupt that ended it, length in us).
const GAPS: usize = 16;
static GAP_END: [[AtomicU64; GAPS]; MAX_CPUS] = [const { [const { AtomicU64::new(0) }; GAPS] }; MAX_CPUS];
static GAP_LEN: [[AtomicU64; GAPS]; MAX_CPUS] = [const { [const { AtomicU64::new(0) }; GAPS] }; MAX_CPUS];
static GAP_NEXT: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];

pub fn timer_popped(tid: u64, deadline_ns: u64) {
    let i = tid as usize % TIDS;
    POP_DEADLINE[i].store(deadline_ns, Ordering::Relaxed);
    POP_AT[i].store(mono_ns(), Ordering::Relaxed);
    POP_CPU[i].store(cpu_now() as u64, Ordering::Relaxed);
    PLACED[i].store(u64::MAX, Ordering::Relaxed);
    DISPATCH_AT[i].store(0, Ordering::Relaxed);
}

pub fn timer_placed(tid: u64, target: Option<usize>, idle: bool) {
    let i = tid as usize % TIDS;
    PLACED[i].store(target.map_or(1000, |t| t as u64 + if idle { 100 } else { 0 }), Ordering::Relaxed);
}

pub fn dispatched(tid: u64, cpu: usize) {
    let i = tid as usize % TIDS;
    if DISPATCH_AT[i].load(Ordering::Relaxed) == 0 && POP_AT[i].load(Ordering::Relaxed) != 0 {
        DISPATCH_AT[i].store(mono_ns(), Ordering::Relaxed);
        DISPATCH_CPU[i].store(cpu as u64, Ordering::Relaxed);
    }
}

const LATES: usize = 8;
static LATE_WRITE: AtomicU64 = AtomicU64::new(0);
static LATE_READ: AtomicU64 = AtomicU64::new(0);
static LATE_REC: [[AtomicU64; 10]; LATES] = [const { [const { AtomicU64::new(0) }; 10] }; LATES];

/// A timed futex wait returning ETIMEDOUT at `now_ns`, deadline `deadline_ns`.
pub fn late_return(tid: u64, deadline_ns: u64, now_ns: u64) {
    if now_ns.saturating_sub(deadline_ns) < 20_000_000 {
        return;
    }
    let i = tid as usize % TIDS;
    let n = LATE_WRITE.fetch_add(1, Ordering::Relaxed);
    if n.saturating_sub(LATE_READ.load(Ordering::Relaxed)) >= LATES as u64 {
        return;
    }
    let r = &LATE_REC[n as usize % LATES];
    let vals = [
        tid,
        deadline_ns,
        now_ns,
        POP_DEADLINE[i].load(Ordering::Relaxed),
        POP_AT[i].load(Ordering::Relaxed),
        POP_CPU[i].load(Ordering::Relaxed),
        PLACED[i].load(Ordering::Relaxed),
        DISPATCH_AT[i].load(Ordering::Relaxed),
        DISPATCH_CPU[i].load(Ordering::Relaxed),
        cpu_now() as u64,
    ];
    for (slot, v) in r.iter().zip(vals) {
        slot.store(v, Ordering::Relaxed);
    }
}

fn print_lates() {
    let rel = |t: u64, base: u64| -> i64 {
        if t == 0 { i64::MIN } else { (t as i64 - base as i64) / 1000 }
    };
    while LATE_READ.load(Ordering::Relaxed) < LATE_WRITE.load(Ordering::Relaxed).min(LATE_READ.load(Ordering::Relaxed) + LATES as u64) {
        let n = LATE_READ.load(Ordering::Relaxed);
        let r = &LATE_REC[n as usize % LATES];
        let v: [u64; 10] = core::array::from_fn(|k| r[k].load(Ordering::Relaxed));
        let (deadline, now) = (v[1], v[2]);
        let mut gaps = [(0i64, 0u64, 0usize); 24];
        let mut count = 0;
        for cpu in 0..crate::arch_impl::x86_64::smp::cpus_online().min(4) as usize {
            for k in 0..GAPS {
                let end = GAP_END[cpu][k].load(Ordering::Relaxed);
                let len = GAP_LEN[cpu][k].load(Ordering::Relaxed);
                if end == 0 || end + 1_000_000 < deadline || end - len * 1000 > now {
                    continue;
                }
                if count < gaps.len() {
                    gaps[count] = (rel(end - len * 1000, deadline), len, cpu);
                    count += 1;
                }
            }
        }
        log::info!(
            "LATEWAKE tid={} late_us={} popdl_match={} pop_us={} popcpu={} placed={} disp_us={} dispcpu={} retcpu={} gaps(start_us,len_us,cpu)={:?}",
            v[0],
            (now - deadline) / 1000,
            v[3] == deadline,
            rel(v[4], deadline),
            v[5],
            v[6] as i64,
            rel(v[7], deadline),
            v[8],
            v[9],
            &gaps[..count],
        );
        LATE_READ.fetch_add(1, Ordering::Relaxed);
    }
}

fn cycles_per_ms() -> u64 {
    (frequency_hz() / 1000).max(1)
}

fn cpu_now() -> usize {
    if crate::per_cpu::is_initialized() {
        crate::per_cpu::cpu_id()
    } else {
        0
    }
}

fn in_window(t0: u64, t1: u64) -> bool {
    let st = STATE.load(Ordering::Acquire);
    if st != ST_OPEN && st != ST_CLOSED {
        return false;
    }
    t0 != 0 && t1 > WSTART.load(Ordering::Acquire) && t0 < WEND.load(Ordering::Acquire)
}

fn note_span(arr: &[AtomicU64; MAX_CPUS], cpu: usize, t0: u64, t1: u64) -> bool {
    if cpu < MAX_CPUS && t1 > t0 && in_window(t0, t1) {
        arr[cpu].fetch_max(t1 - t0, Ordering::Relaxed);
        return true;
    }
    false
}

/// Every timer-entry interrupt (tick, reschedule IPI, retry).
pub fn note_interrupt(cpu: usize) {
    if cpu >= MAX_CPUS {
        return;
    }
    let now = rdtsc();
    let prev = LAST_INT[cpu].swap(now, Ordering::Relaxed);
    note_span(&GAP, cpu, prev, now);
    if prev != 0 && now - prev > 10 * cycles_per_ms() {
        let k = GAP_NEXT[cpu].fetch_add(1, Ordering::Relaxed) as usize % GAPS;
        GAP_LEN[cpu][k].store((now - prev) / (cycles_per_ms() / 1000).max(1), Ordering::Relaxed);
        GAP_END[cpu][k].store(mono_ns(), Ordering::Relaxed);
    }
    if cpu == 0 {
        maybe_arm(now);
    }
}

fn maybe_arm(now: u64) {
    if !FORCING || !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    match STATE.load(Ordering::Acquire) {
        ST_IDLE if now >= NEXT_ARM.load(Ordering::Relaxed) => {
            let kind = ROTATE.fetch_add(1, Ordering::Relaxed) % 4 + 1;
            let kind = if kind == 4 { 2 } else { kind };
            KIND.store(kind, Ordering::Relaxed);
            STATE.store(ST_ARMED, Ordering::Release);
        }
        ST_ARMED if now > NEXT_ARM.load(Ordering::Relaxed) + 2000 * cycles_per_ms() => {
            NEXT_ARM.store(now, Ordering::Relaxed);
            STATE.store(ST_IDLE, Ordering::Release);
        }
        ST_CLOSED if now > WEND.load(Ordering::Acquire).saturating_add(30 * cycles_per_ms()) => {
            STATE.store(ST_DONE, Ordering::Release);
        }
        _ => {}
    }
}

fn try_begin(kind: u32, cpu: usize) -> bool {
    if STATE.load(Ordering::Acquire) != ST_ARMED || KIND.load(Ordering::Relaxed) != kind {
        return false;
    }
    if STATE
        .compare_exchange(ST_ARMED, ST_STARTING, Ordering::AcqRel, Ordering::Relaxed)
        .is_err()
    {
        return false;
    }
    WCPU.store(cpu as u64, Ordering::Relaxed);
    WEND.store(u64::MAX, Ordering::Relaxed);
    let mut flags = 0;
    if SCHED_HOLDER.load(Ordering::Relaxed) == cpu as u64 {
        flags |= 1;
    }
    if crate::process::process_manager_held_on_current_cpu() {
        flags |= 2;
    }
    WFLAG.store(flags, Ordering::Relaxed);
    WSTART.store(rdtsc(), Ordering::Release);
    STATE.store(ST_OPEN, Ordering::Release);
    true
}

fn end() {
    WEND.store(rdtsc(), Ordering::Release);
    STATE.store(ST_CLOSED, Ordering::Release);
}

fn spin_ms(ms: u64) {
    let until = rdtsc() + ms * cycles_per_ms();
    while rdtsc() < until {
        core::hint::spin_loop();
    }
}

/// UNAVAIL: the idle loop, interrupts masked, about to halt.
pub fn idle_before_halt() {
    let cpu = cpu_now();
    if cpu < MAX_CPUS && try_begin(UNAVAIL, cpu) {
        FORCE_NMI[cpu].store(true, Ordering::Release);
        let _ = crate::arch_impl::x86_64::apic::send_ipi(
            crate::arch_impl::x86_64::apic::id(),
            crate::arch_impl::x86_64::apic::Ipi::Nmi,
        );
    }
}

/// Every NMI: record the interrupted RIP and RFLAGS when a sample is wanted.
pub fn nmi_sample(cpu: usize, rip: u64, flags: u64) {
    if cpu < MAX_CPUS && SAMPLE_WANTED[cpu].swap(false, Ordering::AcqRel) {
        SAMPLE_RIP[cpu].store(rip, Ordering::Relaxed);
        SAMPLE_FLAGS[cpu].store(flags, Ordering::Relaxed);
    }
}

/// UNAVAIL: top of the NMI handler.
pub fn nmi_hook(cpu: usize) {
    if cpu < MAX_CPUS && FORCE_NMI[cpu].swap(false, Ordering::AcqRel) {
        spin_ms(if KIND.load(Ordering::Relaxed) == PMHOLD { 50 } else { 100 });
        end();
    }
}

/// PMHOLD: PROCESS_MANAGER was just taken on this CPU.
pub fn pm_hold_hook() {
    if x86_64::instructions::interrupts::are_enabled() {
        return;
    }
    let cpu = cpu_now();
    if cpu < MAX_CPUS && try_begin(PMHOLD, cpu) {
        FORCE_NMI[cpu].store(true, Ordering::Release);
        let _ = crate::arch_impl::x86_64::apic::send_ipi(
            crate::arch_impl::x86_64::apic::id(),
            crate::arch_impl::x86_64::apic::Ipi::Nmi,
        );
    }
}

/// IPI: just before a reschedule-vector APIC write.
pub fn ipi_hook(vector: u8) {
    if vector != crate::interrupts::RESCHEDULE_VECTOR {
        return;
    }
    let cpu = cpu_now();
    if try_begin(IPI, cpu) {
        x86_64::instructions::interrupts::without_interrupts(|| {
            spin_ms(20);
            for other in 0..crate::arch_impl::x86_64::smp::cpus_online().min(4) as usize {
                if other == cpu {
                    continue;
                }
                SAMPLE_RIP[other].store(0, Ordering::Relaxed);
                SAMPLE_WANTED[other].store(true, Ordering::Release);
                if let Some(apic) = crate::arch_impl::x86_64::smp::cpu_apic_id(other) {
                    let _ = crate::arch_impl::x86_64::apic::send_ipi(
                        apic,
                        crate::arch_impl::x86_64::apic::Ipi::Nmi,
                    );
                }
            }
            spin_ms(10);
        });
        end();
    }
}

/// DRAIN: the idle loop's log drain starts (`true`) or returns (`false`).
pub fn idle_drain(active: bool) {
    let cpu = cpu_now();
    if cpu >= MAX_CPUS {
        return;
    }
    IN_IDLE_DRAIN[cpu].store(active, Ordering::Release);
    if !active
        && STATE.load(Ordering::Acquire) == ST_OPEN
        && KIND.load(Ordering::Relaxed) == DRAIN
        && WCPU.load(Ordering::Relaxed) == cpu as u64
    {
        DRAIN_LEFT.store(0, Ordering::Relaxed);
        end();
    }
}

/// DRAIN: each COM2 port write.
pub fn port_write_hook() {
    let cpu = cpu_now();
    if cpu >= MAX_CPUS
        || !IN_IDLE_DRAIN[cpu].load(Ordering::Acquire)
        || !crate::per_cpu::running_idle_thread()
        || crate::per_cpu::in_interrupt()
    {
        return;
    }
    if STATE.load(Ordering::Acquire) == ST_OPEN
        && KIND.load(Ordering::Relaxed) == DRAIN
        && WCPU.load(Ordering::Relaxed) == cpu as u64
    {
        let left = DRAIN_LEFT.load(Ordering::Relaxed);
        if left > 0 {
            spin_ms(5);
            DRAIN_LEFT.store(left - 1, Ordering::Relaxed);
            if left == 1 {
                end();
            }
        }
    } else if try_begin(DRAIN, cpu) {
        DRAIN_LEFT.store(15, Ordering::Relaxed);
        spin_ms(5);
    }
}

/// The interrupt-return dispatch got (`true`) or missed PROCESS_MANAGER.
pub fn dispatch_guard(got: bool) {
    let cpu = cpu_now();
    if cpu >= MAX_CPUS {
        return;
    }
    if got {
        let since = DISPATCH_FAIL_SINCE[cpu].swap(0, Ordering::Relaxed);
        if since != 0 {
            note_span(&DISPATCH_BLOCKED, cpu, since, rdtsc());
        }
    } else if DISPATCH_FAIL_SINCE[cpu].load(Ordering::Relaxed) == 0 {
        DISPATCH_FAIL_SINCE[cpu].store(rdtsc(), Ordering::Relaxed);
    }
}

pub fn sched_acquired(t0: u64) {
    let cpu = crate::task::scheduler::current_cpu_id_raw();
    note_span(&SCHED_WAIT, cpu, t0, rdtsc());
    SCHED_HOLDER.store(cpu as u64, Ordering::Relaxed);
}

pub fn sched_released() {
    SCHED_HOLDER.store(u64::MAX, Ordering::Relaxed);
}

pub fn pm_acquired(t0: u64) {
    note_span(&PM_WAIT, cpu_now(), t0, rdtsc());
}

pub fn shootdown_done(t0: u64, targets: u64, loc: &'static core::panic::Location<'static>) {
    let cpu = cpu_now();
    let t1 = rdtsc();
    if note_span(&SD_WAIT, cpu, t0, t1) && t1 - t0 > SD_MAX.load(Ordering::Relaxed) {
        SD_MAX.store(t1 - t0, Ordering::Relaxed);
        SD_MAX_LOC.store(loc as *const _ as usize, Ordering::Relaxed);
        SD_MAX_TGT.store(targets | ((cpu as u64) << 16), Ordering::Relaxed);
        let mut held = 0;
        if SCHED_HOLDER.load(Ordering::Relaxed) == cpu as u64 {
            held |= 1;
        }
        if crate::process::process_manager_held_on_current_cpu() {
            held |= 2;
        }
        SD_MAX_HELD.store(held, Ordering::Relaxed);
    }
}

/// From kstrandd: start arming, and print a finished window.
pub fn poll() {
    print_lates();
    if !ENABLED.swap(true, Ordering::Relaxed) {
        NEXT_ARM.store(rdtsc() + 1000 * cycles_per_ms(), Ordering::Relaxed);
    }
    if STATE.load(Ordering::Acquire) != ST_DONE {
        return;
    }
    let per_us = (frequency_hz() / 1_000_000).max(1);
    let us = |arr: &[AtomicU64; MAX_CPUS], i: usize| arr[i].load(Ordering::Relaxed) / per_us;
    let n = crate::arch_impl::x86_64::smp::cpus_online().min(4) as usize;
    let row = |arr: &[AtomicU64; MAX_CPUS]| {
        let mut v = [0u64; 4];
        for (i, slot) in v.iter_mut().enumerate().take(n) {
            *slot = us(arr, i);
        }
        v
    };
    let (gap, sched, pm, sd) = (row(&GAP), row(&SCHED_WAIT), row(&PM_WAIT), row(&SD_WAIT));
    let blocked = row(&DISPATCH_BLOCKED);
    let loc = SD_MAX_LOC.load(Ordering::Relaxed);
    let (file, line) = if loc == 0 {
        ("-", 0)
    } else {
        // SAFETY: stored from a `&'static Location<'static>`.
        let loc = unsafe { &*(loc as *const core::panic::Location<'static>) };
        (loc.file(), loc.line())
    };
    let seq = WSEQ.fetch_add(1, Ordering::Relaxed);
    log::info!(
        "SPREAD seq={} kind={} cpu={} dur_us={} held={} gap_us={:?} sched_us={:?} pm_us={:?} sd_us={:?} sd_max_us={} sd_at={}:{} sd_tgt={:#x} sd_held={} dblk_us={:?} rip={:x?} rfl={:x?}",
        seq,
        KIND.load(Ordering::Relaxed),
        WCPU.load(Ordering::Relaxed),
        (WEND.load(Ordering::Relaxed) - WSTART.load(Ordering::Relaxed)) / per_us,
        WFLAG.load(Ordering::Relaxed),
        gap,
        sched,
        pm,
        sd,
        SD_MAX.load(Ordering::Relaxed) / per_us,
        file,
        line,
        SD_MAX_TGT.load(Ordering::Relaxed),
        SD_MAX_HELD.load(Ordering::Relaxed),
        blocked,
        [0, 1, 2, 3].map(|i| SAMPLE_RIP[i].load(Ordering::Relaxed)),
        [0, 1, 2, 3].map(|i| SAMPLE_FLAGS[i].load(Ordering::Relaxed)),
    );
    for slot in SAMPLE_RIP.iter() {
        slot.store(0, Ordering::Relaxed);
    }
    for arr in [&GAP, &SCHED_WAIT, &PM_WAIT, &SD_WAIT, &DISPATCH_BLOCKED] {
        for slot in arr.iter() {
            slot.store(0, Ordering::Relaxed);
        }
    }
    SD_MAX.store(0, Ordering::Relaxed);
    SD_MAX_LOC.store(0, Ordering::Relaxed);
    SD_MAX_TGT.store(0, Ordering::Relaxed);
    SD_MAX_HELD.store(0, Ordering::Relaxed);
    NEXT_ARM.store(rdtsc() + 200 * cycles_per_ms(), Ordering::Relaxed);
    STATE.store(ST_IDLE, Ordering::Release);
}
