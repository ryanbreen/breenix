//! LAPIC one-shot sleeps sharing the calibrated scheduler timer.
//! ARM counterpart: timer_interrupt::request_wakeup advances CNT{P,V}_CVAL.
//! Intel SDM vol. 3, Local APIC timer: LVT mode 00 is one-shot, 01 periodic.
use core::{cell::UnsafeCell, sync::atomic::{AtomicU8, Ordering}};
use super::clockevent::ClockEvent;
const PERIOD_NS: u64 = crate::time::timer::MS_PER_TICK * 1_000_000;
static VECTOR: AtomicU8 = AtomicU8::new(0);
struct Slot(UnsafeCell<ClockEvent>);
// Each slot belongs to one CPU. Requests mask local IRQs; only that CPU's
// timer callback accesses the same slot. No other CPU reads or writes it.
unsafe impl Sync for Slot {}
static EVENTS: [Slot; crate::task::scheduler::MAX_CPUS] =
    [const { Slot(UnsafeCell::new(ClockEvent::new())) }; crate::task::scheduler::MAX_CPUS];
pub(super) fn init() {
    let vector = super::super::msi::reserve(interrupt).expect("clock-event interrupt vector");
    VECTOR.store(vector, Ordering::Release);
}
fn now_ns() -> u64 { crate::signal::monotonic_nanos() }
fn program(deadline: u64) {
    let count = u64::from(super::TIMER_COUNT.load(Ordering::Relaxed));
    let delta = deadline.saturating_sub(now_ns()).max(1).min(PERIOD_NS);
    let ticks = (count * delta).div_ceil(PERIOD_NS).max(1) as u32;
    super::write(0x320, u32::from(VECTOR.load(Ordering::Relaxed)));
    super::write(0x380, ticks);
}
pub fn request_wakeup(deadline_ns: u64) -> bool {
    if VECTOR.load(Ordering::Acquire) == 0 || !super::active() { return false; }
    crate::arch_without_interrupts(|| {
        let cpu = crate::per_cpu::cpu_id();
        let Some(slot) = EVENTS.get(cpu) else { return false; };
        let count = u64::from(super::TIMER_COUNT.load(Ordering::Relaxed));
        if count == 0 { return false; }
        let left = u64::from(super::read(0x390)) * PERIOD_NS / count;
        // SAFETY: this CPU owns the slot and local IRQs are masked.
        let event = unsafe { &mut *slot.0.get() };
        let next = event.request(now_ns(), left, deadline_ns);
        program(next);
        true
    })
}
fn interrupt() {
    let cpu = crate::per_cpu::cpu_id();
    let Some(slot) = EVENTS.get(cpu) else { return; };
    // SAFETY: the local interrupt gate masks IRQs and this CPU owns the slot.
    let now = now_ns();
    let event = unsafe { &mut *slot.0.get() };
    let deadline = event.wake_deadline();
    let (tick, wake, next) = event.interrupt(now, PERIOD_NS, crate::time::wakeup::POLL_NS);
    if wake {
        crate::time::wakeup::record_wake(deadline, now);
    }
    match next { Some(deadline) => program(deadline), None => super::start_local_timer() }
    if tick || wake {
        // The existing software handler charges elapsed counter time and claims
        // due sleep deadlines. A fine-only wake is not a delivered tick.
        // This gate sends the EOI.
        crate::interrupts::timer::timer_interrupt_handler(0, u8::from(!tick));
    }
    if wake {
        crate::task::scheduler::set_need_resched();
    }
    // This gate has no assembly scheduling epilogue. Hand both fine wakes
    // and regular-tick reschedules to the existing retry return path.
    if crate::task::scheduler::is_need_resched() {
        crate::task::scheduler::retry_after_interrupts_x86();
    }
}
