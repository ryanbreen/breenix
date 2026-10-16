//! SCRATCH (#1540/#1635/#1636): make one vCPU unavailable inside the guest,
//! the way a host that stops running it does, and record a timeline for every
//! wait that returns late. Never lands.
//!
//! HOSTSTALL forcing (when `FORCING`): every 300-500 ms an interrupt picks a
//! CPU from the TSC's low bits and sends it an NMI whose handler spins 50 ms,
//! whatever that CPU was doing: in user mode, holding a lock with interrupts
//! masked, or halted. The window records what it interrupted.
//!
//! Every wait's end is recorded per thread: the timer-heap pop of its deadline
//! or the signal wake, the CPU the wake placed it on, its dispatch, and its
//! return to user mode (syscall return or a handler frame). A return more than
//! 20 ms after the wait became due prints one LATE line from kstrandd, with
//! the forcing windows, the silent stretches of each CPU (no timer-entry
//! interrupt for 10 ms or more), and the scheduler-lock and PROCESS_MANAGER
//! waits over 3 ms that overlap it.

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::arch_impl::x86_64::timer::{frequency_hz, rdtsc};
use crate::task::scheduler::MAX_CPUS;

/// 0: record only. 1: HOSTSTALL, a random CPU unavailable 50 ms every
/// 300-500 ms. 2: PMHOLD, a CPU that has just taken PROCESS_MANAGER with
/// interrupts masked unavailable 50 ms, at most every 300-500 ms.
pub const FORCE_KIND: u32 = 2;
const FORCING: bool = FORCE_KIND == 1;
const STALL_MS: u64 = 50;
const LATE_NS: u64 = 10_000_000;

fn cycles_per_us() -> u64 {
    (frequency_hz() / 1_000_000).max(1)
}

fn mono_ns() -> u64 {
    let (s, n) = crate::time::get_monotonic_time_ns();
    s as u64 * 1_000_000_000 + n as u64
}

fn cpu_now() -> usize {
    if crate::per_cpu::is_initialized() {
        crate::per_cpu::cpu_id()
    } else {
        0
    }
}

fn ncpus() -> usize {
    (crate::arch_impl::x86_64::smp::cpus_online() as usize).clamp(1, 4)
}

// ---------------------------------------------------------------- forcing

static ENABLED: AtomicBool = AtomicBool::new(false);
static NEXT_ARM: AtomicU64 = AtomicU64::new(u64::MAX);
static FORCE_NMI: [AtomicBool; MAX_CPUS] = [const { AtomicBool::new(false) }; MAX_CPUS];

/// Forcing windows: start ns, end ns, cpu | flags << 8, interrupted tid.
/// flags: 1 user mode, 2 IF set, 4 held PROCESS_MANAGER, 8 held the
/// scheduler lock, 16 idle thread current.
const WINS: usize = 64;
static WIN_NEXT: AtomicU64 = AtomicU64::new(0);
static WIN: [[AtomicU64; 5]; WINS] = [const { [const { AtomicU64::new(0) }; 5] }; WINS];
/// Windows seen, and how many interrupted each of: user, PM held, sched held, idle.
static WIN_COUNT: [AtomicU64; 5] = [const { AtomicU64::new(0) }; 5];
static WIN_SLOT: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];

pub static SCHED_HOLDER: AtomicU64 = AtomicU64::new(u64::MAX);

/// Silent stretches over 10 ms: end ns, length us, cpu.
const GAPS: usize = 128;
static GAP_NEXT: AtomicU64 = AtomicU64::new(0);
static GAP: [[AtomicU64; 3]; GAPS] = [const { [const { AtomicU64::new(0) }; 3] }; GAPS];
static LAST_INT: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];

/// Lock waits over 3 ms: end ns, length us, cpu | kind << 8 | holder << 16.
/// kind 1 scheduler, 2 PROCESS_MANAGER, 3 futex table, 4 own run-queue lock.
const WAITS: usize = 128;
static WAIT_NEXT: AtomicU64 = AtomicU64::new(0);
static WAIT: [[AtomicU64; 4]; WAITS] = [const { [const { AtomicU64::new(0) }; 4] }; WAITS];

/// Every timer-entry interrupt (tick, reschedule IPI, retry).
pub fn note_interrupt(cpu: usize) {
    if cpu >= MAX_CPUS {
        return;
    }
    let now = rdtsc();
    let prev = LAST_INT[cpu].swap(now, Ordering::Relaxed);
    SILENCE_SAMPLED[cpu].store(false, Ordering::Relaxed);
    for peer in 0..ncpus() {
        let last = LAST_INT[peer].load(Ordering::Relaxed);
        if peer != cpu
            && last != 0
            && now > last
            && now - last > 15_000 * cycles_per_us()
            && !SILENCE_SAMPLED[peer].swap(true, Ordering::Relaxed)
        {
            SAMPLE_SILENT_US[peer].store((now - last) / cycles_per_us(), Ordering::Relaxed);
            request_sample(3, peer);
        }
    }
    if prev != 0 && now > prev && now - prev > 10_000 * cycles_per_us() {
        let k = GAP_NEXT.fetch_add(1, Ordering::Relaxed) as usize % GAPS;
        GAP[k][0].store(mono_ns(), Ordering::Relaxed);
        GAP[k][1].store((now - prev) / cycles_per_us(), Ordering::Relaxed);
        GAP[k][2].store(cpu as u64, Ordering::Relaxed);
    }
    if FORCING && ENABLED.load(Ordering::Relaxed) {
        let next = NEXT_ARM.load(Ordering::Relaxed);
        if now >= next
            && NEXT_ARM
                .compare_exchange(next, u64::MAX, Ordering::AcqRel, Ordering::Relaxed)
                .is_ok()
        {
            let target = ((now >> 7) % ncpus() as u64) as usize;
            FORCE_NMI[target].store(true, Ordering::Release);
            if let Some(apic) = crate::arch_impl::x86_64::smp::cpu_apic_id(target) {
                let _ = crate::arch_impl::x86_64::apic::send_ipi(
                    apic,
                    crate::arch_impl::x86_64::apic::Ipi::Nmi,
                );
            }
        }
    }
}

// ------------------------------------------------- lock-holder sampling

/// A waiter that has spun 20 ms on a lock NMIs its holder once: the holder
/// records its RIP and how long the NMI took to be answered. Microseconds
/// mean the holder was running code; tens of milliseconds mean its vCPU was
/// not executing.
static SAMPLE_REQ: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];
static SAMPLE_WHO: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];
/// How long the sampled CPU had been silent when a kind-3 sample was asked.
static SAMPLE_SILENT_US: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];
/// Set once a silent CPU has been sampled; cleared at its next interrupt.
static SILENCE_SAMPLED: [AtomicBool; MAX_CPUS] = [const { AtomicBool::new(false) }; MAX_CPUS];
const HOLDS: usize = 32;
static HOLD_WRITE: AtomicU64 = AtomicU64::new(0);
static HOLD_READ: AtomicU64 = AtomicU64::new(0);
/// at ns, answer us, holder | kind << 8 | waiter << 16 | flags << 24, rip,
/// silent us (kind 3), then up to six kernel-text words from the interrupted
/// kernel stack.
static HOLD: [[AtomicU64; 11]; HOLDS] = [const { [const { AtomicU64::new(0) }; 11] }; HOLDS];

fn request_sample(kind: u64, holder: usize) {
    if holder >= MAX_CPUS || holder == cpu_now() {
        return;
    }
    SAMPLE_WHO[holder].store(kind | (cpu_now() as u64) << 8, Ordering::Relaxed);
    if SAMPLE_REQ[holder]
        .compare_exchange(0, rdtsc(), Ordering::AcqRel, Ordering::Relaxed)
        .is_err()
    {
        return;
    }
    if let Some(apic) = crate::arch_impl::x86_64::smp::cpu_apic_id(holder) {
        let _ = crate::arch_impl::x86_64::apic::send_ipi(apic, crate::arch_impl::x86_64::apic::Ipi::Nmi);
    }
}

/// Spin until `try_once` succeeds, sampling the holder `holder()` names once
/// the wait passes 20 ms.
pub fn spin_acquire<T>(kind: u64, holder: impl Fn() -> u64, mut try_once: impl FnMut() -> Option<T>) -> T {
    let t0 = rdtsc();
    let mut sampled = false;
    loop {
        if let Some(guard) = try_once() {
            return guard;
        }
        if !sampled && rdtsc().saturating_sub(t0) > 20_000 * cycles_per_us() {
            sampled = true;
            request_sample(kind, holder() as usize);
        }
        core::hint::spin_loop();
    }
}

/// Per CPU: when it took its own run-queue lock (0 while it holds none),
/// total cycles held, holds, the longest hold.
static RQ_HOLD_AT: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];
static RQ_HOLD_SUM: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];
static RQ_HOLD_N: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];
static RQ_HOLD_MAX: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];
/// Try-locks of a peer's run-queue lock: taken, refused.
static RQ_TRY: [AtomicU64; 2] = [const { AtomicU64::new(0) }; 2];

/// This CPU waited `t0`..now for its own run-queue lock (kind 4).
pub fn rq_waited(t0: u64) {
    note_wait(4, t0, 0xff, 0);
}

/// A wait for another CPU's run-queue lock (`rq::with_thread`): kind 5.
pub fn owner_waited(t0: u64, owner: usize) {
    note_wait(5, t0, owner as u64, 0);
}

/// Paths taken: 0 kill lock-free, 1 kill PROCESS_MANAGER, 2 sigtimedwait
/// accept lock-free, 3 its accept under PM, 4 fd lookup lock-free, 5 fd
/// lookup under PM, 6 timer syscall without PM, 7 sigtimedwait registration
/// lock-free, 8 its registration under PM, 9 held wakes.
static FAST: [AtomicU64; 10] = [const { AtomicU64::new(0) }; 10];

pub fn fast(i: usize) {
    if i < FAST.len() {
        FAST[i].fetch_add(1, Ordering::Relaxed);
    }
}

pub fn rq_acquired(cpu: usize) {
    if cpu < MAX_CPUS {
        RQ_HOLD_AT[cpu].store(rdtsc(), Ordering::Relaxed);
    }
}

pub fn rq_released(cpu: usize) {
    if cpu < MAX_CPUS {
        let at = RQ_HOLD_AT[cpu].swap(0, Ordering::Relaxed);
        if at != 0 {
            let held = rdtsc().saturating_sub(at);
            RQ_HOLD_SUM[cpu].fetch_add(held, Ordering::Relaxed);
            RQ_HOLD_N[cpu].fetch_add(1, Ordering::Relaxed);
            RQ_HOLD_MAX[cpu].fetch_max(held, Ordering::Relaxed);
        }
    }
}

pub fn rq_tried(taken: bool) {
    RQ_TRY[!taken as usize].fetch_add(1, Ordering::Relaxed);
}

pub fn sched_holder() -> u64 {
    SCHED_HOLDER.load(Ordering::Relaxed)
}

pub fn pm_holder() -> u64 {
    crate::process::process_manager_owner_snapshot().map_or(u64::MAX, |(cpu, _)| cpu)
}

/// Top of the NMI handler.
pub fn nmi_hook(cpu: usize, rip: u64, cs: u64, rflags: u64, rsp: u64) {
    if cpu < MAX_CPUS {
        let sent = SAMPLE_REQ[cpu].load(Ordering::Acquire);
        if sent != 0 {
            let who = SAMPLE_WHO[cpu].load(Ordering::Relaxed);
            let flags = (cs & 3 == 3) as u64 | ((rflags & 0x200 != 0) as u64) << 1;
            let n = HOLD_WRITE.fetch_add(1, Ordering::Relaxed);
            if n.saturating_sub(HOLD_READ.load(Ordering::Relaxed)) < HOLDS as u64 {
                let r = &HOLD[n as usize % HOLDS];
                r[0].store(mono_ns(), Ordering::Relaxed);
                r[1].store(rdtsc().saturating_sub(sent) / cycles_per_us(), Ordering::Relaxed);
                r[2].store(cpu as u64 | (who & 0xff) << 8 | (who >> 8 & 0xff) << 16 | flags << 24, Ordering::Relaxed);
                r[3].store(rip, Ordering::Relaxed);
                r[4].store(SAMPLE_SILENT_US[cpu].load(Ordering::Relaxed), Ordering::Relaxed);
                for slot in &r[5..] {
                    slot.store(0, Ordering::Relaxed);
                }
                // A page-aligned RSP is an empty stack: its page is the
                // unmapped one above the stack's top.
                if cs & 3 == 0 && rsp & 7 == 0 && rsp & 0xfff != 0 {
                    // Kernel text lies within a few MB of this function.
                    let here = nmi_hook as usize as u64;
                    let (lo, hi) = (here.saturating_sub(8 << 20), here + (8 << 20));
                    let end = (rsp & !0xfff) + 0x1000;
                    let mut found = 0;
                    let mut at = rsp;
                    while at < end && found < 6 {
                        // SAFETY: the words from RSP to the end of its page
                        // are on the interrupted kernel stack, which is mapped.
                        let word = unsafe { core::ptr::read_volatile(at as *const u64) };
                        if word >= lo && word < hi {
                            r[5 + found].store(word, Ordering::Relaxed);
                            found += 1;
                        }
                        at += 8;
                    }
                }
            }
            SAMPLE_REQ[cpu].store(0, Ordering::Release);
        }
    }
    if cpu >= MAX_CPUS || !FORCE_NMI[cpu].swap(false, Ordering::AcqRel) {
        return;
    }
    let mut flags = 0u64;
    if cs & 3 == 3 {
        flags |= 1;
    }
    if rflags & 0x200 != 0 {
        flags |= 2;
    }
    if crate::process::process_manager_held_on_current_cpu() {
        flags |= 4;
    }
    if SCHED_HOLDER.load(Ordering::Relaxed) == cpu as u64 || RQ_HOLD_AT[cpu.min(MAX_CPUS - 1)].load(Ordering::Relaxed) != 0 {
        flags |= 8;
    }
    if crate::per_cpu::running_idle_thread() {
        flags |= 16;
    }
    let tid = crate::per_cpu::current_thread_id_lock_free().unwrap_or(u64::MAX);
    let k = WIN_NEXT.fetch_add(1, Ordering::Relaxed) as usize % WINS;
    let start = mono_ns();
    WIN[k][0].store(start, Ordering::Relaxed);
    WIN[k][1].store(0, Ordering::Relaxed);
    WIN[k][2].store(cpu as u64 | flags << 8, Ordering::Relaxed);
    WIN[k][3].store(tid, Ordering::Relaxed);
    WIN[k][4].store(rip, Ordering::Relaxed);
    WIN_COUNT[0].fetch_add(1, Ordering::Relaxed);
    for (bit, slot) in [(1u64, 1usize), (4, 2), (8, 3), (16, 4)] {
        if flags & bit != 0 {
            WIN_COUNT[slot].fetch_add(1, Ordering::Relaxed);
        }
    }
    WIN_SLOT[cpu].store(k as u64, Ordering::Relaxed);
    for (other, slot) in WIN_DISP_START.iter().enumerate() {
        slot.store(DISP_COUNT[other].load(Ordering::Relaxed), Ordering::Relaxed);
    }
    let t0 = rdtsc();
    let until = t0 + STALL_MS * 1000 * cycles_per_us();
    while rdtsc() < until {
        core::hint::spin_loop();
    }
    WIN[k][1].store(mono_ns(), Ordering::Relaxed);
    let mut others = 0;
    for other in (0..ncpus()).filter(|&other| other != cpu) {
        others += DISP_COUNT[other].load(Ordering::Relaxed).saturating_sub(WIN_DISP_START[other].load(Ordering::Relaxed));
    }
    WIN_DISP[0].fetch_add(1, Ordering::Relaxed);
    WIN_DISP[1].fetch_add(others, Ordering::Relaxed);
    if others == 0 {
        WIN_DISP[2].fetch_add(1, Ordering::Relaxed);
    }
    WIN_DISP[3].fetch_add((rdtsc() - t0) / cycles_per_us(), Ordering::Relaxed);
    let gap = (300 + (rdtsc() >> 9) % 200) * 1000 * cycles_per_us();
    NEXT_ARM.store(rdtsc() + gap, Ordering::Release);
}

/// PMHOLD: PROCESS_MANAGER was just taken on this CPU.
pub fn pm_hold_hook() {
    if FORCE_KIND != 2 || !ENABLED.load(Ordering::Relaxed) || x86_64::instructions::interrupts::are_enabled() {
        return;
    }
    let cpu = cpu_now();
    let now = rdtsc();
    let next = NEXT_ARM.load(Ordering::Relaxed);
    if cpu < MAX_CPUS
        && now >= next
        && NEXT_ARM
            .compare_exchange(next, u64::MAX, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
    {
        FORCE_NMI[cpu].store(true, Ordering::Release);
        let _ = crate::arch_impl::x86_64::apic::send_ipi(
            crate::arch_impl::x86_64::apic::id(),
            crate::arch_impl::x86_64::apic::Ipi::Nmi,
        );
    }
}

fn note_wait(kind: u64, t0: u64, holder: u64, site: u64) {
    let t1 = rdtsc();
    if t1 <= t0 || t1 - t0 < 3_000 * cycles_per_us() {
        return;
    }
    let k = WAIT_NEXT.fetch_add(1, Ordering::Relaxed) as usize % WAITS;
    WAIT[k][0].store(mono_ns(), Ordering::Relaxed);
    WAIT[k][1].store((t1 - t0) / cycles_per_us(), Ordering::Relaxed);
    WAIT[k][2].store(cpu_now() as u64 | kind << 8 | (holder & 0xff) << 16, Ordering::Relaxed);
    WAIT[k][3].store(site, Ordering::Relaxed);
}

pub fn sched_wait_begin() -> (u64, u64) {
    (rdtsc(), SCHED_HOLDER.load(Ordering::Relaxed))
}

/// Per CPU: when it took the scheduler lock, total cycles held, holds, and
/// the longest hold in cycles.
static SCHED_HOLD_AT: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];
static SCHED_HOLD_SUM: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];
static SCHED_HOLD_N: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];
static SCHED_HOLD_MAX: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];
static PROBE_T0: AtomicU64 = AtomicU64::new(0);

const SITES: usize = 128;
static SITE_KEY: [AtomicU64; SITES] = [const { AtomicU64::new(0) }; SITES];
static SITE_N: [AtomicU64; SITES] = [const { AtomicU64::new(0) }; SITES];
static SITE_CYC: [AtomicU64; SITES] = [const { AtomicU64::new(0) }; SITES];
static SITE_MAX: [AtomicU64; SITES] = [const { AtomicU64::new(0) }; SITES];
static HOLD_SITE: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];

fn site_slot(key: u64) -> Option<usize> {
    let start = (key >> 3) as usize % SITES;
    for k in 0..SITES {
        let i = (start + k) % SITES;
        let cur = SITE_KEY[i].load(Ordering::Relaxed);
        if cur == key {
            return Some(i);
        }
        if cur == 0 {
            match SITE_KEY[i].compare_exchange(0, key, Ordering::AcqRel, Ordering::Relaxed) {
                Ok(_) => return Some(i),
                Err(now) if now == key => return Some(i),
                Err(_) => continue,
            }
        }
    }
    None
}

pub fn sched_acquired_at(begin: (u64, u64), site: &'static core::panic::Location<'static>) {
    let cpu = crate::task::scheduler::current_cpu_id_raw();
    if cpu < MAX_CPUS {
        HOLD_SITE[cpu].store(site as *const _ as u64, Ordering::Relaxed);
    }
    sched_acquired(begin);
}

pub fn sched_acquired(begin: (u64, u64)) {
    let cpu = crate::task::scheduler::current_cpu_id_raw();
    note_wait(1, begin.0, begin.1, 0);
    SCHED_HOLDER.store(cpu as u64, Ordering::Relaxed);
    if cpu < MAX_CPUS {
        SCHED_HOLD_AT[cpu].store(rdtsc(), Ordering::Relaxed);
    }
}

pub fn sched_released() {
    let cpu = crate::task::scheduler::current_cpu_id_raw();
    if cpu < MAX_CPUS {
        let at = SCHED_HOLD_AT[cpu].swap(0, Ordering::Relaxed);
        if at != 0 {
            let held = rdtsc().saturating_sub(at);
            SCHED_HOLD_SUM[cpu].fetch_add(held, Ordering::Relaxed);
            SCHED_HOLD_N[cpu].fetch_add(1, Ordering::Relaxed);
            SCHED_HOLD_MAX[cpu].fetch_max(held, Ordering::Relaxed);
            let site = HOLD_SITE[cpu].swap(0, Ordering::Relaxed);
            if let Some(i) = (site != 0).then(|| site_slot(site)).flatten() {
                SITE_N[i].fetch_add(1, Ordering::Relaxed);
                SITE_CYC[i].fetch_add(held, Ordering::Relaxed);
                SITE_MAX[i].fetch_max(held, Ordering::Relaxed);
            }
        }
    }
    SCHED_HOLDER.store(u64::MAX, Ordering::Relaxed);
}

static FUTEX_HOLDER: AtomicU64 = AtomicU64::new(u64::MAX);

/// Futex table waits are kind 3, with the holder's CPU.
pub fn futex_wait_begin() -> (u64, u64) {
    (rdtsc(), FUTEX_HOLDER.load(Ordering::Relaxed))
}

#[track_caller]
pub fn futex_acquired(begin: (u64, u64)) {
    note_wait(3, begin.0, begin.1, core::panic::Location::caller() as *const _ as u64);
    FUTEX_HOLDER.store(cpu_now() as u64, Ordering::Relaxed);
}

pub fn futex_released() {
    FUTEX_HOLDER.store(u64::MAX, Ordering::Relaxed);
}

pub fn pm_wait_begin() -> (u64, u64) {
    let holder = crate::process::process_manager_owner_snapshot().map_or(0xff, |(cpu, _)| cpu);
    (rdtsc(), holder)
}

#[track_caller]
pub fn pm_acquired(begin: (u64, u64)) {
    pm_acquired_at(begin, core::panic::Location::caller());
}

pub fn pm_acquired_at(begin: (u64, u64), site: &'static core::panic::Location<'static>) {
    note_wait(2, begin.0, begin.1, site as *const _ as u64);
}

/// A PROCESS_MANAGER wait's call site, printed as file:line.
#[derive(Clone, Copy)]
struct Site(u64);

impl core::fmt::Debug for Site {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        if self.0 == 0 {
            return f.write_str("-");
        }
        let loc = unsafe { &*(self.0 as *const core::panic::Location<'static>) };
        write!(f, "{}:{}", loc.file().rsplit('/').next().unwrap_or(""), loc.line())
    }
}

// ---------------------------------------------------------------- waits

const TIDS: usize = 1024;
/// Per tid: when the wait became due (deadline, or signal wake), kind
/// (1 timer pop, 2 signal wake), when the pop or wake ran and on which CPU,
/// the CPU it was placed on (+100 when no CPU was chosen), dispatch time
/// and CPU.
static DUE: [AtomicU64; TIDS] = [const { AtomicU64::new(0) }; TIDS];
static KIND: [AtomicU64; TIDS] = [const { AtomicU64::new(0) }; TIDS];
static WAKE_AT: [AtomicU64; TIDS] = [const { AtomicU64::new(0) }; TIDS];
static WAKE_CPU: [AtomicU64; TIDS] = [const { AtomicU64::new(0) }; TIDS];
static PLACED: [AtomicU64; TIDS] = [const { AtomicU64::new(0) }; TIDS];
static DISP_AT: [AtomicU64; TIDS] = [const { AtomicU64::new(0) }; TIDS];
static DISP_CPU: [AtomicU64; TIDS] = [const { AtomicU64::new(0) }; TIDS];

/// The timer heap popped `tid`'s live deadline.
pub fn timer_due(tid: u64, deadline_ns: u64) {
    let i = tid as usize % TIDS;
    DUE[i].store(deadline_ns, Ordering::Relaxed);
    KIND[i].store(1, Ordering::Relaxed);
    WAKE_AT[i].store(mono_ns(), Ordering::Relaxed);
    WAKE_CPU[i].store(cpu_now() as u64, Ordering::Relaxed);
    PLACED[i].store(u64::MAX, Ordering::Relaxed);
    DISP_AT[i].store(0, Ordering::Relaxed);
}

/// A signal woke `tid`; `due_ns` is when the signal became due when known.
pub fn signal_due(tid: u64, due_ns: Option<u64>) {
    let i = tid as usize % TIDS;
    let now = mono_ns();
    if KIND[i].load(Ordering::Relaxed) == 2 && DUE[i].load(Ordering::Relaxed) != 0 {
        return;
    }
    DUE[i].store(due_ns.unwrap_or(now), Ordering::Relaxed);
    KIND[i].store(2, Ordering::Relaxed);
    WAKE_AT[i].store(now, Ordering::Relaxed);
    WAKE_CPU[i].store(cpu_now() as u64, Ordering::Relaxed);
    PLACED[i].store(u64::MAX, Ordering::Relaxed);
    DISP_AT[i].store(0, Ordering::Relaxed);
}

pub fn placed(tid: u64, target: Option<usize>) {
    let i = tid as usize % TIDS;
    if DUE[i].load(Ordering::Relaxed) != 0 {
        PLACED[i].store(target.map_or(100, |t| t as u64), Ordering::Relaxed);
    }
}

/// Placement with what the target CPU held: its current tid, queue length,
/// whether a promoted (head) enqueue was open, whether it was idle, and the
/// current thread's rank.
static PLACE_INFO: [AtomicU64; TIDS] = [const { AtomicU64::new(0) }; TIDS];

pub fn placed_info(tid: u64, target: Option<usize>, info: u64) {
    placed(tid, target);
    PLACE_INFO[tid as usize % TIDS].store(info, Ordering::Relaxed);
}

/// Dispatches per CPU, and per window the other CPUs' dispatches inside it.
static DISP_COUNT: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];
static WIN_DISP_START: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];
/// windows, sum of other-CPU dispatches in them, windows with none, sum of
/// window lengths us; and dispatches and time outside windows.
static WIN_DISP: [AtomicU64; 4] = [const { AtomicU64::new(0) }; 4];
static FIRST_DISP_TSC: AtomicU64 = AtomicU64::new(0);

pub fn dispatched(tid: u64, cpu: usize) {
    if cpu < MAX_CPUS {
        DISP_COUNT[cpu].fetch_add(1, Ordering::Relaxed);
        if FIRST_DISP_TSC.load(Ordering::Relaxed) == 0 {
            FIRST_DISP_TSC.store(rdtsc(), Ordering::Relaxed);
        }
    }
    let i = tid as usize % TIDS;
    if DUE[i].load(Ordering::Relaxed) != 0 && DISP_AT[i].load(Ordering::Relaxed) == 0 {
        DISP_AT[i].store(mono_ns(), Ordering::Relaxed);
        DISP_CPU[i].store(cpu as u64, Ordering::Relaxed);
    }
}

const LATES: usize = 64;
static LATE_WRITE: AtomicU64 = AtomicU64::new(0);
static LATE_READ: AtomicU64 = AtomicU64::new(0);
static LATE_REC: [[AtomicU64; 11]; LATES] = [const { [const { AtomicU64::new(0) }; 11] }; LATES];

/// The current thread returns to user mode (syscall return, or a signal
/// handler frame installed; `site` 1 or 2).
pub fn returned(site: u64) {
    let Some(tid) = crate::per_cpu::current_thread_id_lock_free() else {
        return;
    };
    let i = tid as usize % TIDS;
    let due = DUE[i].swap(0, Ordering::Relaxed);
    if due == 0 {
        return;
    }
    let now = mono_ns();
    if now < due || now - due < LATE_NS {
        return;
    }
    let n = LATE_WRITE.fetch_add(1, Ordering::Relaxed);
    if n.saturating_sub(LATE_READ.load(Ordering::Relaxed)) >= LATES as u64 {
        return;
    }
    let vals = [
        tid,
        KIND[i].load(Ordering::Relaxed) | site << 8,
        due,
        WAKE_AT[i].load(Ordering::Relaxed),
        WAKE_CPU[i].load(Ordering::Relaxed),
        PLACED[i].load(Ordering::Relaxed),
        DISP_AT[i].load(Ordering::Relaxed),
        DISP_CPU[i].load(Ordering::Relaxed),
        now,
        cpu_now() as u64,
        PLACE_INFO[i].swap(0, Ordering::Relaxed),
    ];
    for (slot, v) in LATE_REC[n as usize % LATES].iter().zip(vals) {
        slot.store(v, Ordering::Relaxed);
    }
}

fn rel_ms(t: u64, base: u64) -> i64 {
    if t == 0 {
        return i64::MIN;
    }
    (t as i64 - base as i64) / 100_000
}

static NEXT_SUMMARY: AtomicU64 = AtomicU64::new(0);

/// Every 10 s: windows by what they interrupted, silent stretches per CPU
/// (count, longest us) and lock waits (count, longest us) since boot.
fn summary() {
    let now = rdtsc();
    let next = NEXT_SUMMARY.load(Ordering::Relaxed);
    if now < next {
        return;
    }
    NEXT_SUMMARY.store(now + 10_000_000 * cycles_per_us(), Ordering::Relaxed);
    let mut gaps = [(0u64, 0u64); 4];
    for g in GAP.iter() {
        let (e, len, cpu) = (g[0].load(Ordering::Relaxed), g[1].load(Ordering::Relaxed), g[2].load(Ordering::Relaxed) as usize);
        if e != 0 && cpu < 4 {
            gaps[cpu].0 += 1;
            gaps[cpu].1 = gaps[cpu].1.max(len);
        }
    }
    let mut waits = [(0u64, 0u64); 2];
    for w in WAIT.iter() {
        let (e, len, kind) = (w[0].load(Ordering::Relaxed), w[1].load(Ordering::Relaxed), (w[2].load(Ordering::Relaxed) >> 8) & 0xff);
        if e != 0 && (1..=2).contains(&kind) {
            waits[kind as usize - 1].0 += 1;
            waits[kind as usize - 1].1 = waits[kind as usize - 1].1.max(len);
        }
    }
    if PROBE_T0.load(Ordering::Relaxed) == 0 {
        PROBE_T0.store(rdtsc(), Ordering::Relaxed);
    }
    let since_us = rdtsc().saturating_sub(PROBE_T0.load(Ordering::Relaxed)) / cycles_per_us();
    let per = cycles_per_us();
    log::info!(
        "SCHEDHOLD since_us={} held_us={:?} holds={:?} max_us={:?}",
        since_us,
        [0, 1, 2, 3].map(|c| SCHED_HOLD_SUM[c].load(Ordering::Relaxed) / per),
        [0, 1, 2, 3].map(|c| SCHED_HOLD_N[c].load(Ordering::Relaxed)),
        [0, 1, 2, 3].map(|c| SCHED_HOLD_MAX[c].load(Ordering::Relaxed) / per),
    );
    let mut top = [(0u64, 0usize); 24];
    for i in 0..SITES {
        let cyc = SITE_CYC[i].load(Ordering::Relaxed);
        if let Some(slot) = top.iter().position(|(c, _)| cyc > *c) {
            let len = top.len();
            top.copy_within(slot..len - 1, slot + 1);
            top[slot] = (cyc, i);
        }
    }
    let sites = top.map(|(cyc, i)| {
        (Site(SITE_KEY[i].load(Ordering::Relaxed)), SITE_N[i].load(Ordering::Relaxed), cyc / per, SITE_MAX[i].load(Ordering::Relaxed) / per)
    });
    log::info!("SCHEDSITES (site,holds,us,max_us)={:?}", sites);
    log::info!(
        "RQHOLD held_us={:?} holds={:?} max_us={:?} remote_tries(taken,refused)={:?}",
        [0, 1, 2, 3].map(|c| RQ_HOLD_SUM[c].load(Ordering::Relaxed) / per),
        [0, 1, 2, 3].map(|c| RQ_HOLD_N[c].load(Ordering::Relaxed)),
        [0, 1, 2, 3].map(|c| RQ_HOLD_MAX[c].load(Ordering::Relaxed) / per),
        RQ_TRY.each_ref().map(|c| c.load(Ordering::Relaxed)),
    );
    let total: u64 = DISP_COUNT.iter().take(ncpus()).map(|c| c.load(Ordering::Relaxed)).sum();
    let elapsed_us = rdtsc().saturating_sub(FIRST_DISP_TSC.load(Ordering::Relaxed)) / cycles_per_us();
    log::info!(
        "WINDISP windows={} other_cpu_dispatches_in_windows={} windows_with_none={} window_us={} all_dispatches={} elapsed_us={} cpus={}",
        WIN_DISP[0].load(Ordering::Relaxed),
        WIN_DISP[1].load(Ordering::Relaxed),
        WIN_DISP[2].load(Ordering::Relaxed),
        WIN_DISP[3].load(Ordering::Relaxed),
        total,
        elapsed_us,
        ncpus(),
    );
    log::info!(
        "STALLSUM wins(all,user,pm,sched,idle)={:?} gaps_last128(cpu:count,max_us)={:?} waits_last128(sched,pm:count,max_us)={:?} total_gaps={} total_waits={}",
        WIN_COUNT.each_ref().map(|c| c.load(Ordering::Relaxed)),
        gaps,
        waits,
        GAP_NEXT.load(Ordering::Relaxed),
        WAIT_NEXT.load(Ordering::Relaxed),
    );
    let mut long = [(0u64, 0u64, 0u64, Site(0)); 16];
    let mut n = 0;
    for k in 0..WAITS {
        let len_us = WAIT[k][1].load(Ordering::Relaxed);
        if len_us >= 20_000 && n < long.len() {
            long[n] = (
                WAIT[k][0].load(Ordering::Relaxed) / 1_000_000,
                len_us,
                WAIT[k][2].load(Ordering::Relaxed),
                Site(WAIT[k][3].load(Ordering::Relaxed)),
            );
            n += 1;
        }
    }
    log::info!("LONGWAITS (end_ms,len_us,cpu|kind<<8|holder<<16,site)={:?}", &long[..n]);
    let mut kinds = [(0u64, 0u64); 5];
    for w in WAIT.iter() {
        let (e, len, kind) = (w[0].load(Ordering::Relaxed), w[1].load(Ordering::Relaxed), (w[2].load(Ordering::Relaxed) >> 8) & 0xff);
        if e != 0 && (1..=5).contains(&kind) {
            kinds[kind as usize - 1].0 += 1;
            kinds[kind as usize - 1].1 = kinds[kind as usize - 1].1.max(len);
        }
    }
    log::info!(
        "WAITKINDS last128(sched,pm,futex,own_rq,owner_rq:count,max_us)={:?} FAST(kill_lf,kill_pm,stw_lf,stw_pm,fd_lf,fd_pm,timer_lf,stwreg_lf,stwreg_pm,held)={:?}",
        kinds,
        FAST.each_ref().map(|c| c.load(Ordering::Relaxed)),
    );
}

/// From kstrandd: start forcing, and print late records.
pub fn poll() {
    summary();
    while HOLD_READ.load(Ordering::Relaxed)
        < HOLD_WRITE.load(Ordering::Relaxed).min(HOLD_READ.load(Ordering::Relaxed) + HOLDS as u64)
    {
        let n = HOLD_READ.load(Ordering::Relaxed);
        let v: [u64; 11] = core::array::from_fn(|k| HOLD[n as usize % HOLDS][k].load(Ordering::Relaxed));
        log::info!(
            "HOLDSAMPLE at_ms={} answer_us={} holder={} kind={} waiter={} user={} if={} rip={:#x} silent_us={} stack={:x?}",
            v[0] / 1_000_000,
            v[1],
            v[2] & 0xff,
            v[2] >> 8 & 0xff,
            v[2] >> 16 & 0xff,
            v[2] >> 24 & 1,
            v[2] >> 25 & 1,
            v[3],
            v[4],
            &v[5..],
        );
        HOLD_READ.fetch_add(1, Ordering::Relaxed);
    }
    if !ENABLED.swap(true, Ordering::Relaxed) {
        NEXT_ARM.store(rdtsc() + 1_000_000 * cycles_per_us(), Ordering::Release);
    }
    while LATE_READ.load(Ordering::Relaxed)
        < LATE_WRITE.load(Ordering::Relaxed).min(LATE_READ.load(Ordering::Relaxed) + LATES as u64)
    {
        let n = LATE_READ.load(Ordering::Relaxed);
        let v: [u64; 11] = core::array::from_fn(|k| LATE_REC[n as usize % LATES][k].load(Ordering::Relaxed));
        let (due, now) = (v[2], v[8]);
        let lo = due.saturating_sub(5_000_000);
        // Times are tenths of a millisecond from `due`.
        let mut wins = [(0i64, 0i64, 0u64, 0u64, 0u64); 8];
        let mut nw = 0;
        for w in WIN.iter() {
            let (s, e) = (w[0].load(Ordering::Relaxed), w[1].load(Ordering::Relaxed));
            let e2 = if e == 0 { now } else { e };
            if s == 0 || e2 < lo || s > now || nw == wins.len() {
                continue;
            }
            wins[nw] = (rel_ms(s, due), rel_ms(e2, due), w[2].load(Ordering::Relaxed), w[3].load(Ordering::Relaxed), w[4].load(Ordering::Relaxed));
            nw += 1;
        }
        let mut gaps = [(0i64, 0u64, 0u64); 12];
        let mut ng = 0;
        for g in GAP.iter() {
            let (e, len) = (g[0].load(Ordering::Relaxed), g[1].load(Ordering::Relaxed));
            if e == 0 || e < lo || e.saturating_sub(len * 1000) > now || ng == gaps.len() {
                continue;
            }
            gaps[ng] = (rel_ms(e - len * 1000, due), len / 100, g[2].load(Ordering::Relaxed));
            ng += 1;
        }
        let mut waits = [(0i64, 0u64, 0u64, Site(0)); 12];
        let mut nwt = 0;
        for w in WAIT.iter() {
            let (e, len) = (w[0].load(Ordering::Relaxed), w[1].load(Ordering::Relaxed));
            if e == 0 || e < lo || e.saturating_sub(len * 1000) > now || nwt == waits.len() {
                continue;
            }
            waits[nwt] = (rel_ms(e - len * 1000, due), len / 100, w[2].load(Ordering::Relaxed), Site(w[3].load(Ordering::Relaxed)));
            nwt += 1;
        }
        log::info!(
            "LATE tid={} kind={:#x} due_ms10={} late_ms10={} wake=({},{}) placed={} disp=({},{}) ret_cpu={} win(s,e,cpu|fl<<8,tid,rip)={:x?} gap(s,len,cpu)={:?} wait(s,len,cpu|k<<8|h<<16)={:x?} pinfo(cur,q,promo,idle,rank)=({},{},{},{},{})",
            v[0],
            v[1],
            due / 100_000,
            (now - due) / 100_000,
            rel_ms(v[3], due),
            v[4],
            v[5] as i64,
            rel_ms(v[6], due),
            v[7],
            v[9],
            &wins[..nw],
            &gaps[..ng],
            &waits[..nwt],
            v[10] & 0xfffff,
            v[10] >> 20 & 0xff,
            v[10] >> 28 & 1,
            v[10] >> 29 & 1,
            v[10] >> 32,
        );
        LATE_READ.fetch_add(1, Ordering::Relaxed);
    }
}
