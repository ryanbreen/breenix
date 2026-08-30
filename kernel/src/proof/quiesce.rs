//! Quiescence — how much of the machine the component gets to itself.
//!
//! * `Pen` (default): the driver runs the component in a tight loop while every
//!   other online CPU is parked on a harness-owned atomic gate. Parked CPUs
//!   still take interrupts — controlled, not stopped — and the pen releases
//!   between iterations at a PRNG-selected cadence.
//! * `Adversarial`: the parked CPUs instead run the component's paired
//!   operation from the same seeded stream.
//! * `Ambient`: no pen at all; the boot proceeds and the loop rides along.
//!   Deliberately the least sensitive mode, and a mandatory confirmation arm —
//!   a perturbation harness can manufacture its own bug, which the strand
//!   oracle recorded in its own source (7 aborts and 4 hangs in 49 boots with
//!   injection fully disarmed but its loop running, versus 1 abort in 50
//!   without the loop). A finding is not a finding until it survives `Ambient`.
//!
//! ## Every wait is bounded, and giving up is recorded
//!
//! The rendezvous words below are waited on with a spin. An unbounded spin is
//! the wrong failure: if a peer kthread is spawned but never dispatched, the
//! driver would hang holding a CPU, the boot would produce no RUN record, and
//! the gate would report "missing RUN" — true, but pointing at the harness
//! instead of at anything about the kernel. Each wait therefore has a bounded
//! budget, and exhausting it degrades the run to `Ambient` and says so in the
//! run record rather than hanging or pretending the pen formed.

use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

use crate::task::kthread::KthreadHandle;

use super::coverage::{self, MutSite};
use super::rng::{self, AntagonistOp};

const RELEASED: u64 = 0;
const PARKED: u64 = 1;
const STOPPED: u64 = 2;

static CONTROL: AtomicU64 = AtomicU64::new(RELEASED);
static ACTIVE_CPUS: AtomicU64 = AtomicU64::new(0);
static CLEAR_OBSERVED: AtomicU64 = AtomicU64::new(0);
static EXITED_CPUS: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Pen,
    Adversarial,
    Ambient,
}

impl Mode {
    /// The build-selected default is per-component: Pen for A (unchanged —
    /// round 3's own measurement is that Pen alone drives Component A's probe
    /// fine), and Adversarial for C, because Pen suppresses the very
    /// condition Component C hunts (see `driver_c.rs` and rung 2's spec,
    /// section 2, for why). An explicit `BREENIX_COREPROOF_MODE` always wins
    /// over either default.
    pub fn selected() -> Self {
        match option_env!("BREENIX_COREPROOF_MODE") {
            Some("adversarial") => Self::Adversarial,
            Some("ambient") => Self::Ambient,
            Some("pen") => Self::Pen,
            _ if cfg!(feature = "coreproof_component_c") => Self::Adversarial,
            _ => Self::Pen,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Pen => "pen",
            Self::Adversarial => "adversarial",
            Self::Ambient => "ambient",
        }
    }
}

/// Which boot interval the measured loop occupies.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Window {
    PostCohort,
    Overlap,
}

impl Window {
    pub fn selected(mode: Mode) -> Self {
        match option_env!("BREENIX_COREPROOF_WINDOW") {
            Some("post_cohort") => Self::PostCohort,
            Some("overlap") => Self::Overlap,
            _ if mode == Mode::Ambient => Self::Overlap,
            _ => Self::PostCohort,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::PostCohort => "post_cohort",
            Self::Overlap => "overlap",
        }
    }
}

/// Spin budget for one rendezvous, in iterations of the wait loop.
///
/// Sized to be generous for a rendezvous that normally completes in
/// microseconds and still bounded well inside one boot.
const RENDEZVOUS_SPIN_BUDGET: u64 = 200_000_000;

/// Set when any rendezvous exhausted its budget. The run record reports it, so
/// a degraded run is never read as a clean penned one.
static DEGRADED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Whether any rendezvous gave up and degraded the run to ambient.
pub fn degraded() -> bool {
    DEGRADED.load(Ordering::Relaxed)
}

pub struct Controller {
    mask: u64,
    handles: Vec<KthreadHandle>,
}

impl Controller {
    pub fn begin(
        mode: Mode,
        window: Window,
        root_seed: u64,
        component: u8,
        driver_cpu: usize,
        victim_tid: u64,
        online_cpus: usize,
    ) -> Self {
        if mode == Mode::Ambient || online_cpus <= 1 {
            return Self {
                mask: 0,
                handles: Vec::new(),
            };
        }

        ACTIVE_CPUS.store(0, Ordering::Release);
        CLEAR_OBSERVED.store(0, Ordering::Release);
        EXITED_CPUS.store(0, Ordering::Release);
        CONTROL.store(PARKED, Ordering::Release);

        let mut mask = 0u64;
        let mut handles = Vec::new();
        for cpu in 0..online_cpus.min(crate::arch_impl::aarch64::smp::MAX_CPUS) {
            if cpu == driver_cpu {
                continue;
            }
            let bit = 1u64 << cpu;
            let worker_mode = mode;
            let result = crate::task::kthread::kthread_run_on_cpu_for_test(
                move || {
                    worker(
                        worker_mode,
                        window,
                        root_seed,
                        component,
                        cpu,
                        victim_tid,
                        online_cpus,
                    )
                },
                "coreproof-peer",
                cpu,
            );
            if let Ok(handle) = result {
                mask |= bit;
                handles.push(handle);
            }
        }

        if !wait_for_mask(&ACTIVE_CPUS, mask) {
            // The pen never formed. Release whatever did start and run ambient
            // rather than spinning on a CPU that is not coming.
            CONTROL.store(STOPPED, Ordering::Release);
            return Self { mask: 0, handles };
        }
        Self { mask, handles }
    }

    pub fn release_and_reform(&self) {
        if self.mask == 0 {
            return;
        }

        CONTROL.store(RELEASED, Ordering::Release);
        if !wait_for_mask(&CLEAR_OBSERVED, self.mask) {
            return;
        }
        let mut spins = 0u64;
        while ACTIVE_CPUS.load(Ordering::Acquire) & self.mask != 0 {
            core::hint::spin_loop();
            spins += 1;
            if spins >= RENDEZVOUS_SPIN_BUDGET {
                DEGRADED.store(true, Ordering::Relaxed);
                return;
            }
        }

        CLEAR_OBSERVED.fetch_and(!self.mask, Ordering::AcqRel);
        CONTROL.store(PARKED, Ordering::Release);
        let _ = wait_for_mask(&ACTIVE_CPUS, self.mask);
    }

    /// Stop every peer and wait for it to leave, so no parked CPU outlives the
    /// run. A peer that outlives the driver is a harness bug, not a finding.
    pub fn finish(self) {
        // Stop unconditionally: `begin` may have degraded to mask 0 while peers
        // were already running, and those peers still have to be told to leave.
        CONTROL.store(STOPPED, Ordering::Release);
        if self.mask != 0 {
            let _ = wait_for_mask(&CLEAR_OBSERVED, self.mask);
            let _ = wait_for_mask(&EXITED_CPUS, self.mask);
        }
        for handle in self.handles {
            let _ = crate::task::kthread::kthread_join(&handle);
        }
    }
}

/// Spin until every bit in `mask` is set, or the budget runs out.
///
/// Returns `false` when the budget was exhausted, having recorded that the run
/// is degraded. Callers must treat `false` as "the pen is not there" rather than
/// retrying.
#[must_use]
fn wait_for_mask(word: &AtomicU64, mask: u64) -> bool {
    let mut spins = 0u64;
    while word.load(Ordering::Acquire) & mask != mask {
        core::hint::spin_loop();
        spins += 1;
        if spins >= RENDEZVOUS_SPIN_BUDGET {
            DEGRADED.store(true, Ordering::Relaxed);
            return false;
        }
    }
    true
}

fn adversarial_step(
    root_seed: u64,
    component: u8,
    cpu: usize,
    iteration: u64,
    victim_tid: u64,
    online_cpus: usize,
    state: &mut PeerState,
) {
    let vector = rng::draw(root_seed, component, cpu as u8, iteration);
    match vector.antagonist_op {
        AntagonistOp::Unblock => {
            let _ =
                crate::task::scheduler::with_scheduler(|scheduler| scheduler.unblock(victim_tid));
        }
        AntagonistOp::Placement if !state.placement_exercised => {
            // This wrapper publishes through spawn_on_cpu_for_test, so the
            // placement protocol is exercised without exposing a raw Thread.
            let online = online_cpus.min(crate::arch_impl::aarch64::smp::MAX_CPUS);
            let target = usize::from(vector.antagonist_cpu) % online.max(1);
            let _ =
                crate::task::kthread::kthread_run_on_cpu_for_test(|| {}, "coreproof-place", target);
            state.placement_exercised = true;
        }
        AntagonistOp::Placement => core::hint::spin_loop(),
        AntagonistOp::KernelSchedule => kernel_schedule(),
        AntagonistOp::ThreadChurn => {
            let cadence = u64::from(vector.cycles % THREAD_CHURN_CADENCE_SPAN) + 1;
            if state.churn_spawned < THREAD_CHURN_CAP && iteration % cadence == 0 {
                if let Ok(handle) =
                    crate::task::kthread::kthread_run_on_cpu_for_test(|| {}, "coreproof-churn", cpu)
                {
                    state.churn_spawned += 1;
                    if !state.pending_next_exercised {
                        state.pending_next_exercised =
                            crate::task::scheduler::with_scheduler(|scheduler| {
                                scheduler.exercise_pending_next_coreproof_probe(handle.tid())
                            })
                            .unwrap_or(false);
                    }
                }
            }
        }
        AntagonistOp::ReclaimDrain => {
            crate::arch_impl::aarch64::context_switch::run_deferred_reclamation();
        }
        AntagonistOp::Steal => {
            // From a foreign CPU, make the victim eligible if it is blocked and
            // immediately drive the real local-first/then-steal pick path. If
            // the victim is still current elsewhere this remains an attempt,
            // which is exactly the production admission rule under test.
            let _ =
                crate::task::scheduler::with_scheduler(|scheduler| scheduler.unblock(victim_tid));
            kernel_schedule();
        }
    }
}

const THREAD_CHURN_CAP: u32 = 32;
const THREAD_CHURN_CADENCE_SPAN: u32 = 32;

struct PeerState {
    placement_exercised: bool,
    churn_spawned: u32,
    pending_next_exercised: bool,
}

fn kernel_schedule() {
    crate::task::scheduler::schedule();
    coverage::note(MutSite::CpuIdentity);
}

fn worker(
    mode: Mode,
    window: Window,
    root_seed: u64,
    component: u8,
    cpu: usize,
    victim_tid: u64,
    online_cpus: usize,
) {
    let bit = 1u64 << cpu;
    let mut iteration = 0u64;
    let mut state = PeerState {
        placement_exercised: false,
        churn_spawned: 0,
        pending_next_exercised: false,
    };

    loop {
        match CONTROL.load(Ordering::Acquire) {
            PARKED => {
                ACTIVE_CPUS.fetch_or(bit, Ordering::AcqRel);
                if mode == Mode::Adversarial {
                    adversarial_step(
                        root_seed,
                        component,
                        cpu,
                        iteration,
                        victim_tid,
                        online_cpus,
                        &mut state,
                    );
                    iteration = iteration.wrapping_add(1);
                } else {
                    core::hint::spin_loop();
                }
                if window == Window::Overlap {
                    crate::task::scheduler::yield_current();
                }
            }
            RELEASED => {
                ACTIVE_CPUS.fetch_and(!bit, Ordering::AcqRel);
                CLEAR_OBSERVED.fetch_or(bit, Ordering::AcqRel);
                while CONTROL.load(Ordering::Acquire) == RELEASED {
                    if window == Window::Overlap {
                        crate::task::scheduler::yield_current();
                    }
                    core::hint::spin_loop();
                }
            }
            _ => {
                ACTIVE_CPUS.fetch_and(!bit, Ordering::AcqRel);
                CLEAR_OBSERVED.fetch_or(bit, Ordering::AcqRel);
                EXITED_CPUS.fetch_or(bit, Ordering::Release);
                return;
            }
        }
    }
}
