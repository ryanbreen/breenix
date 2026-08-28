mod driver_a;
mod quiesce;
mod record;
mod rng;
mod sites;
mod stimulus;

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};

pub use quiesce::Mode;
pub use rng::{draw, AntagonistOp, DrawVector, Order, Xorshift64Star};
pub use sites::{SiteClass, SiteId, ALL, DECLARED};
pub use stimulus::Action;

const DISARMED: u64 = 0;

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

    let _ = crate::task::kthread::kthread_run_on_cpu_for_test(driver_a::run, "coreproof-a", 0);
}
