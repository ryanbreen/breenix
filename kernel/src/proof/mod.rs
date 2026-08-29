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
//!    contributes three new predicates; everything else is a read of a counter
//!    that already fails a gate. A parallel truth that disagrees with the
//!    original would be worse than no check.
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
//! relaxed load and a compare after the first visit), one relaxed load of this
//! CPU's armed word, one compare, and a predicted-not-taken branch out of line.
//! The perturbation itself is `#[cold]` and `#[inline(never)]`, so the seam adds
//! a few instructions to the masked critical sections it sits in and no more.
//!
//! An arm is one-iteration, one-hit: `fire` consumes the slot before applying an
//! action, so an action that reschedules cannot let unrelated work reach the
//! same seam and fire the same vector twice.
//!
//! ## Scope of this pilot
//!
//! AArch64 only, and component A only. The x86 driver and the remaining seven
//! components are later rungs, each of which names its own additions in its own
//! PR. `mutations` carries the register of planted defects the harness is
//! validated against, and states up front how a miss is to be read.

pub mod coverage;
mod driver_a;
pub mod mutations;
mod quiesce;
mod record;
mod rng;
mod sites;
mod stimulus;

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};

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

pub(crate) fn arm(vector: &DrawVector) {
    let Some(slot) = ARMED.get(current_cpu()) else {
        return;
    };
    slot.action.store(vector.action as u8, Ordering::Relaxed);
    slot.ticks.store(vector.ticks, Ordering::Relaxed);
    slot.cycles.store(vector.cycles, Ordering::Relaxed);
    slot.antagonist_op
        .store(vector.antagonist_op as u8, Ordering::Relaxed);
    slot.antagonist_cpu
        .store(vector.antagonist_cpu, Ordering::Relaxed);
    slot.order.store(vector.order as u8, Ordering::Relaxed);
    slot.site
        .store(u64::from(vector.site as u8) + 1, Ordering::Release);
}

pub(crate) fn disarm() {
    if let Some(slot) = ARMED.get(current_cpu()) {
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
    let armed_site = slot.site.load(Ordering::Relaxed);
    if armed_site == u64::from(site as u8) + 1 {
        fire(slot, site);
    }
}

#[inline(never)]
#[cold]
fn fire(slot: &ArmedSlot, site: SiteId) {
    // A vector is a one-iteration, one-hit arm. Consume it before applying an
    // action that may reschedule and let unrelated work reach the same seam.
    slot.site.store(DISARMED, Ordering::Relaxed);
    let vector = DrawVector {
        site,
        action: Action::from_u8(slot.action.load(Ordering::Relaxed)),
        ticks: slot.ticks.load(Ordering::Relaxed),
        cycles: slot.cycles.load(Ordering::Relaxed),
        antagonist_op: AntagonistOp::from_u8(slot.antagonist_op.load(Ordering::Relaxed)),
        antagonist_cpu: slot.antagonist_cpu.load(Ordering::Relaxed),
        order: Order::from_u8(slot.order.load(Ordering::Relaxed)),
    };
    stimulus::apply(&vector);
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
    let _ = crate::task::kthread::kthread_run(driver_a::run, "coreproof-a");
}
