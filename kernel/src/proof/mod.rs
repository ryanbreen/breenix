//! The core-proof harness — one kernel component, driven hard and scored.
//!
//! An in-kernel torture loop that rides a REAL boot at `PostScheduler`, against
//! the component's real callees and the real per-CPU state its postconditions
//! read. Not a dedicated boot mode: a mode would buy nothing the pen buys and
//! would cost the arguability of every catch, because the component would then
//! be running against a kernel state no production boot reaches.
//!
//! Four properties are non-negotiable, and each is a mechanism here rather than
//! a promise:
//!
//! 1. **Test profile only.** The module is compiled solely under `coreproof`,
//!    which implies `boot_tests`. The `proof_point!` seam macro lives in
//!    `lib.rs` in two polarities and expands to LITERALLY NOTHING without the
//!    feature — not an empty call the optimiser is trusted to remove.
//!    `scripts/check-coreproof-seams.sh` scans the source and
//!    `tests/coreproof_production_clean.rs` scans the binary, including a
//!    `.text` byte-comparison against a seam-stripped tree.
//! 2. **Seeded and reproducible.** Every draw is a pure function of
//!    `(root_seed, component, cpu, iteration)`, and the seed is on the wire
//!    before the first iteration. See `rng` for what a seed does and does not
//!    replay on a four-CPU multi-threaded TCG guest — the answer is a
//!    measurement, not a caveat.
//! 3. **Postconditions read existing markers.** The component's own contract
//!    contributes a handful of new predicates; everything else is a read of a
//!    counter that already fails a gate. A parallel truth that disagrees with
//!    the original would be worse than no check.
//! 4. **No seam in a Tier-1 file, in any interrupt or syscall handler, or in
//!    the ERET epilogue.** Permanent exclusions. The timer is reached only as a
//!    stimulus source through the already-public `timer::arm_timer`, called from
//!    harness code — never as a seam in a handler.
//!
//! ## Two independent confirmation arms
//!
//! `BREENIX_COREPROOF_DISARM=1` removes only the PERTURBATION: the loop still
//! makes the same draws, probes at the same cadence, forms the same pen and runs
//! the same antagonist workload, but it never calls `arm`, so no seam fires and
//! `stimulus::apply` cannot run. `Ambient` removes only the PEN: injection stays
//! live while the full machine runs. Design risk 1 requires both because a
//! finding that survives a disarmed pen may still be caused by the pen, while a
//! finding that survives ambient with injection live may still be caused by the
//! injection.
//!
//! ## The fast path
//!
//! `seam()` is what every `proof_point!` expands to: mark the site visited (a
//! relaxed load and a compare after the first visit), one Acquire load of this
//! CPU's armed word, one compare, and a predicted-not-taken branch out of line.
//! The perturbation itself is `#[cold]` and `#[inline(never)]`, so the seam adds
//! a few instructions to the masked critical sections it sits in and no more.
//!
//! An arm is one-iteration, one-hit: `fire` consumes the slot before applying an
//! action, so an action that reschedules cannot let unrelated work reach the
//! same seam and fire the same vector twice.
//!
//! `arm`/`disarm` act on the CALLING cpu's own slot — Component A's whole
//! stimulus is self-armed, synchronous within the driver's own probe call.
//! Component C's is not: the defect it hunts needs the seam to fire on a PEER
//! cpu's execution of `scheduler::schedule()`, not the driver's own, so
//! `arm_cpu`/`disarm_cpu` below let the driver target any online cpu's slot
//! from wherever it happens to be running. `ARMED` already being a per-CPU
//! array was necessary but not sufficient for that genuine cross-CPU handoff:
//! rung 2 adds the actual Acquire/Release pairing and the seqlock splice-window
//! close below. Rung 1's writer and reader were always the same cpu and never
//! needed either mechanism.
//!
//! ## Scope
//!
//! AArch64 only. Rung 1 shipped Component A (the ready-queue departure
//! protocol); rung 2 added Component C (per-CPU identity + stack custody), and
//! rung 3 adds Component H (dispatch admission). They are mutually exclusive
//! drivers selected by positive per-component features — see `sites` for why
//! the site census also has to be component-scoped, and `record` for why the
//! marker lines thread a `component` byte instead of hardcoding `comp=A`. The
//! x86 driver and the remaining components are later rungs, each of which names
//! its own additions in its own PR. `mutations` carries the register of planted
//! defects the harness is validated against, and states up front how a miss is
//! to be read.

pub mod coverage;
#[cfg(feature = "coreproof_component_a")]
mod driver_a;
#[cfg(feature = "coreproof_component_c")]
mod driver_c;
#[cfg(feature = "coreproof_component_h")]
mod driver_h;
pub mod mutations;
mod quiesce;
mod record;
mod rng;
mod sites;
mod stimulus;

use core::sync::atomic::{fence, AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};

pub use quiesce::{Mode, Window};
pub use rng::{draw, AntagonistOp, DrawVector, Order, Xorshift64Star};
pub use sites::{SiteClass, SiteId, ALL, DECLARED};
pub use stimulus::Action;

const DISARMED: u64 = 0;

pub(crate) fn loop_disarmed() -> bool {
    matches!(option_env!("BREENIX_COREPROOF_DISARM"), Some("1"))
}

struct ArmedSlot {
    site: AtomicU64,
    /// Seqlock write-in-progress/generation counter. Even = stable, odd = a write is
    /// currently in flight. Bumped by `arm_cpu` (the sole writer of a given slot) before AND
    /// after every payload write, so a concurrent `fire()` on the peer that owns this slot
    /// can detect — and drop rather than apply — a read that overlapped a rewrite in either
    /// direction. See `arm_cpu` and `fire` below; this closes the cross-CPU splice window the
    /// rung 2 review found (M2). Rung 1 never needed this: the writer and reader were always
    /// the same cpu, and program order alone made the read coherent.
    generation: AtomicU64,
    action: AtomicU8,
    ticks: AtomicU64,
    cycles: AtomicU32,
    antagonist_op: AtomicU8,
    antagonist_cpu: AtomicU8,
    order: AtomicU8,
}

impl ArmedSlot {
    const fn new() -> Self {
        Self {
            site: AtomicU64::new(DISARMED),
            generation: AtomicU64::new(0),
            action: AtomicU8::new(Action::None as u8),
            ticks: AtomicU64::new(0),
            cycles: AtomicU32::new(0),
            antagonist_op: AtomicU8::new(AntagonistOp::Unblock as u8),
            antagonist_cpu: AtomicU8::new(0),
            order: AtomicU8::new(Order::Before as u8),
        }
    }
}

/// The vector actually applied by the most recent successful `fire()` on each cpu —
/// advisory record-keeping only, NEVER consulted by `fire`/`seam` themselves. See M3 in the
/// rung 2 review: a violation record naming a fresh, unrelated draw instead of the vector
/// that actually fired has no causal link to the finding it accompanies. This record has its
/// own generation counter because a final `seq` publish alone is not a snapshot protocol: a
/// reader that acquired the old `seq` could otherwise overlap the NEXT fire's field writes
/// before that next writer published its new `seq`, splicing two individually valid fires
/// into one impossible report. As with `ArmedSlot`, a racing reader drops an in-flight sample
/// instead of spinning; a slightly stale but internally coherent past fire is still honest
/// advisory evidence.
struct LastFired {
    /// Even = stable, odd = a record write is in flight. One writer per cpu.
    generation: AtomicU64,
    /// 0 = never fired. Monotonic per-cpu (one writer per slot), so a caller comparing
    /// sequence numbers across several peer cpus can tell which one fired most recently.
    seq: AtomicU64,
    site: AtomicU8,
    action: AtomicU8,
    ticks: AtomicU64,
    cycles: AtomicU32,
    antagonist_op: AtomicU8,
    antagonist_cpu: AtomicU8,
    order: AtomicU8,
}

impl LastFired {
    const fn new() -> Self {
        Self {
            generation: AtomicU64::new(0),
            seq: AtomicU64::new(0),
            site: AtomicU8::new(0),
            action: AtomicU8::new(Action::None as u8),
            ticks: AtomicU64::new(0),
            cycles: AtomicU32::new(0),
            antagonist_op: AtomicU8::new(AntagonistOp::Unblock as u8),
            antagonist_cpu: AtomicU8::new(0),
            order: AtomicU8::new(Order::Before as u8),
        }
    }
}

static ARMED: [ArmedSlot; crate::arch_impl::aarch64::smp::MAX_CPUS] =
    [const { ArmedSlot::new() }; crate::arch_impl::aarch64::smp::MAX_CPUS];
static LAST_FIRED: [LastFired; crate::arch_impl::aarch64::smp::MAX_CPUS] =
    [const { LastFired::new() }; crate::arch_impl::aarch64::smp::MAX_CPUS];
static FIRE_SEQ: AtomicU64 = AtomicU64::new(0);
static STARTED: AtomicBool = AtomicBool::new(false);

/// Set when the boot-test cohort has published its verdict.
///
/// The driver will not form its pen until this is true. See
/// `note_boot_tests_complete`.
static BOOT_TESTS_COMPLETE: AtomicBool = AtomicBool::new(false);

/// The boot-test cohort has published its verdict; the harness may run.
///
/// Called from both of the executor's verdict sites. The harness pens the other
/// online CPUs and runs a tight loop on its own, which is exactly the load that
/// starves a concurrently-running test — and it did: the first smoke boot turned
/// `census_widen_oracle` red, because that oracle needs a QUIET strand census
/// for its baseline and places its probe on the CPU the driver was occupying.
/// The oracle was right and the harness was wrong. This is the strand oracle's
/// recorded hazard in its most concrete form: a perturbation loop can
/// manufacture its own failure, and the answer is to stop overlapping the thing
/// being disturbed, not to relax the thing that noticed.
pub fn note_boot_tests_complete() {
    BOOT_TESTS_COMPLETE.store(true, Ordering::Release);
}

/// Whether the cohort has finished.
pub(crate) fn boot_tests_complete() -> bool {
    BOOT_TESTS_COMPLETE.load(Ordering::Acquire)
}

fn current_cpu() -> usize {
    crate::arch_impl::aarch64::percpu::Aarch64PerCpu::cpu_id() as usize
}

// Self-arm: only Component A's driver uses this shape (it arms its own cpu right before its
// synchronous probe). Components C and H both arm PEER cpus exclusively via `arm_cpu` — see
// the module header — so this would be dead code in either peer-armed build.
#[cfg(feature = "coreproof_component_a")]
pub(crate) fn arm(vector: &DrawVector) {
    arm_cpu(current_cpu(), vector);
}

/// Arm an ARBITRARY online cpu's slot, not necessarily the calling one.
///
/// Component C's stimulus needs to arm a PEER cpu ahead of that peer's own
/// execution of the seam, from the driver's own cpu. See the module header.
pub(crate) fn arm_cpu(cpu: usize, vector: &DrawVector) {
    let Some(slot) = ARMED.get(cpu) else {
        return;
    };
    // Seqlock write side (M2): bump to an odd (in-progress) generation BEFORE touching any
    // payload field, write the payload, then bump to the next even (stable) generation and
    // arm `site` last. A concurrent `fire()` on the peer that samples `generation` before and
    // after its own read sees a mismatch — or an odd value outright — for ANY write that
    // overlaps its read window in either direction, not just one that starts after the read
    // does. There is exactly one writer per slot (this driver, running single-threaded on its
    // own cpu), so a plain load+store pair (not a RMW) is enough to advance it.
    let current_generation = slot.generation.load(Ordering::Relaxed);
    slot.generation
        .store(current_generation.wrapping_add(1), Ordering::Release);
    // A release store only orders accesses that precede it; this full fence also keeps every
    // payload store below from becoming visible before the odd in-progress generation.
    fence(Ordering::SeqCst);
    slot.action.store(vector.action as u8, Ordering::Relaxed);
    slot.ticks.store(vector.ticks, Ordering::Relaxed);
    slot.cycles.store(vector.cycles, Ordering::Relaxed);
    slot.antagonist_op
        .store(vector.antagonist_op as u8, Ordering::Relaxed);
    slot.antagonist_cpu
        .store(vector.antagonist_cpu, Ordering::Relaxed);
    slot.order.store(vector.order as u8, Ordering::Relaxed);
    slot.generation
        .store(current_generation.wrapping_add(2), Ordering::Release);
    // Acquire-paired with `seam()`'s load below: every payload write above, and the final
    // (stable) generation store, happen-before a peer's `seam()` call that observes THIS
    // store via an Acquire load — the pairing rung 1 never needed because the writer and
    // reader were always the same cpu there.
    slot.site
        .store(u64::from(vector.site as u8) + 1, Ordering::Release);
}

// See `arm`'s doc for why this is Component-A-only.
#[cfg(feature = "coreproof_component_a")]
pub(crate) fn disarm() {
    disarm_cpu(current_cpu());
}

/// Disarm an arbitrary online cpu's slot. See `arm_cpu`.
pub(crate) fn disarm_cpu(cpu: usize) {
    if let Some(slot) = ARMED.get(cpu) {
        slot.site.store(DISARMED, Ordering::Release);
    }
}

/// Fast path for every labelled protocol seam.
#[inline(always)]
pub fn seam(site: SiteId) {
    sites::mark_visited(site);
    let Some(slot) = ARMED.get(current_cpu()) else {
        return;
    };
    // Acquire, paired with `arm_cpu`'s final Release store of `site` above — see that
    // function's doc and the module header's cross-CPU note (M2 in the rung 2 review).
    let armed_site = slot.site.load(Ordering::Acquire);
    if armed_site == u64::from(site as u8) + 1 {
        fire(slot, site);
    }
}

#[inline(never)]
#[cold]
fn fire(slot: &ArmedSlot, site: SiteId) {
    let expected_site = u64::from(site as u8) + 1;

    // Seqlock read side (M2): the driver rewrites every peer's slot every iteration
    // regardless of whether the previous arm was consumed (see `driver_c.rs`'s own doc), so a
    // plain field-by-field read here could read some fields from THIS draw and some from the
    // NEXT one. Reading `generation` before and after the payload, and requiring both to
    // agree (and be even — no write in flight), detects any interleaving `arm_cpu` call: it
    // is a single-shot check, not a spin-retry loop, so a detected tear is DROPPED rather than
    // retried. A dropped draw is a lost sample — `iters` already accounts for unfired arms and
    // this is no different. An applied, spliced draw would violate this harness's own "every
    // draw is a pure function of (seed, component, cpu, iteration)" guarantee, which matters.
    let generation_before = slot.generation.load(Ordering::Acquire);
    if generation_before & 1 != 0 {
        // A write is already in flight; the payload is definitely not stable yet. Don't even
        // start reading it.
        return;
    }
    let vector = DrawVector {
        site,
        action: Action::from_u8(slot.action.load(Ordering::Relaxed)),
        ticks: slot.ticks.load(Ordering::Relaxed),
        cycles: slot.cycles.load(Ordering::Relaxed),
        antagonist_op: AntagonistOp::from_u8(slot.antagonist_op.load(Ordering::Relaxed)),
        antagonist_cpu: slot.antagonist_cpu.load(Ordering::Relaxed),
        order: Order::from_u8(slot.order.load(Ordering::Relaxed)),
    };
    // Pair with the writer-side fence: payload loads must finish before the validation load
    // below, otherwise the compiler or cpu could move one past the generation check.
    fence(Ordering::SeqCst);
    let generation_after = slot.generation.load(Ordering::Acquire);
    if generation_after != generation_before {
        // A re-arm landed mid-read (torn) or has already fully replaced this arm. Drop rather
        // than risk applying a spliced vector.
        return;
    }

    // Best-effort one-hit consumption. A full re-arm can land between `seam()`'s site load
    // and `fire()`'s first generation load, so this invocation may read a NEWER draw while
    // still being called from the OLDER call site; the slot's newly armed site can differ
    // from `expected_site`, making this CAS fail. The draw remains safe to apply because
    // `vector.site` is built above from `fire`'s `site` parameter — the call site actually
    // reached — rather than from any slot payload a re-arm could replace. Consequently
    // `stimulus::apply` always judges Masked-vs-Open admissibility from the actual call site.
    // A failed CAS simply leaves the newer arm for the peer's next visit.
    let _ = slot.site.compare_exchange(
        expected_site,
        DISARMED,
        Ordering::Relaxed,
        Ordering::Relaxed,
    );

    record_last_fired(site, &vector);
    stimulus::apply(&vector);
}

/// Record a just-validated, about-to-be-applied fire. Called from `fire()` only, on the cpu
/// whose own slot this is (i.e. `current_cpu()` inside `fire()` is the right index).
fn record_last_fired(site: SiteId, vector: &DrawVector) {
    let Some(last) = LAST_FIRED.get(current_cpu()) else {
        return;
    };
    let seq = FIRE_SEQ.fetch_add(1, Ordering::Relaxed) + 1;
    let current_generation = last.generation.load(Ordering::Relaxed);
    last.generation
        .store(current_generation.wrapping_add(1), Ordering::Release);
    fence(Ordering::SeqCst);
    last.site.store(site as u8, Ordering::Relaxed);
    last.action.store(vector.action as u8, Ordering::Relaxed);
    last.ticks.store(vector.ticks, Ordering::Relaxed);
    last.cycles.store(vector.cycles, Ordering::Relaxed);
    last.antagonist_op
        .store(vector.antagonist_op as u8, Ordering::Relaxed);
    last.antagonist_cpu
        .store(vector.antagonist_cpu, Ordering::Relaxed);
    last.order.store(vector.order as u8, Ordering::Relaxed);
    last.seq.store(seq, Ordering::Release);
    last.generation
        .store(current_generation.wrapping_add(2), Ordering::Release);
}

/// Read one cpu's advisory fire record as a coherent single-shot snapshot. An in-progress or
/// overlapping write is skipped, never retried: violation scoring can use another peer's
/// stable record or the explicit never-fired placeholder instead of delaying a live kernel.
#[cfg(any(
    feature = "coreproof_component_c",
    feature = "coreproof_component_h"
))]
fn last_fired_snapshot(last: &LastFired) -> Option<(u64, DrawVector)> {
    let generation_before = last.generation.load(Ordering::Acquire);
    if generation_before & 1 != 0 {
        return None;
    }
    let seq = last.seq.load(Ordering::Acquire);
    if seq == 0 {
        return None;
    }
    let vector = DrawVector {
        site: sites::SiteId::from_u8(last.site.load(Ordering::Relaxed)),
        action: Action::from_u8(last.action.load(Ordering::Relaxed)),
        ticks: last.ticks.load(Ordering::Relaxed),
        cycles: last.cycles.load(Ordering::Relaxed),
        antagonist_op: AntagonistOp::from_u8(last.antagonist_op.load(Ordering::Relaxed)),
        antagonist_cpu: last.antagonist_cpu.load(Ordering::Relaxed),
        order: Order::from_u8(last.order.load(Ordering::Relaxed)),
    };
    fence(Ordering::SeqCst);
    let generation_after = last.generation.load(Ordering::Acquire);
    if generation_after != generation_before {
        return None;
    }
    Some((seq, vector))
}

/// The most recently fired vector across the given peer cpus, if any has fired at least once,
/// together with the cpu it fired on and that fire's own sequence number. Cross-CPU drivers
/// must thread BOTH values through to `record::violation` as `fired_cpu`/`fired_iter`, rather
/// than pairing a finding with an unrelated fresh draw (M3, rung 2 review; m2, rung 3 review).
/// Components C and H use this bridge; Component A already knows its applied vector directly
/// and synchronously.
#[cfg(any(
    feature = "coreproof_component_c",
    feature = "coreproof_component_h"
))]
pub(crate) fn most_recent_fired(
    peers: impl Iterator<Item = usize>,
) -> Option<(usize, u64, DrawVector)> {
    let mut best: Option<(usize, u64, DrawVector)> = None;
    for cpu in peers {
        let Some(last) = LAST_FIRED.get(cpu) else {
            continue;
        };
        let Some((seq, vector)) = last_fired_snapshot(last) else {
            continue;
        };
        if best.as_ref().is_none_or(|(_, best_seq, _)| seq > *best_seq) {
            best = Some((cpu, seq, vector));
        }
    }
    best
}

pub fn start() {
    if STARTED
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
        .is_err()
    {
        return;
    }

    // Spawned through the ORDINARY placement path, deliberately not
    // `kthread_run_on_cpu_for_test`. A CPU-affine spawn pins the driver to one
    // CPU's ready queue for its whole life, including the stretch where it is
    // only waiting for the boot-test cohort — and a thread sitting Ready on a
    // CPU that is not dispatching is precisely what `census_widen_oracle`'s
    // baseline requires to be absent. The pinned spawn made the harness's own
    // idle driver look like the strand that oracle exists to detect. The driver
    // does not need affinity: it reads its CPU once the pen has formed, and the
    // pen is what keeps it there.
    //
    // Which driver is spawned is a compile-time choice, mutually exclusive with
    // the site census and the mode default it carries — see the module header.
    #[cfg(feature = "coreproof_component_a")]
    let _ = crate::task::kthread::kthread_run(driver_a::run, "coreproof-a");
    #[cfg(feature = "coreproof_component_c")]
    let _ = crate::task::kthread::kthread_run(driver_c::run, "coreproof-c");
    #[cfg(feature = "coreproof_component_h")]
    let _ = crate::task::kthread::kthread_run(driver_h::run, "coreproof-h");
}
