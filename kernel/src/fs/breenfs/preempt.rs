//! Cooperative CPU boundaries for the commit path (E24 ledger 488).
//!
//! Call only outside spin guards and device request ownership. Sleeping mount
//! exclusion may remain held: contenders park. Keep the single entry brake
//! for the drivers, releasing it only while the scheduler saves this thread.

#[inline]
pub(super) fn preemption_point() {
    #[cfg(test)]
    context::point();
    #[cfg(any(target_os = "none", target_os = "breenix", test))]
    if context::need_resched() && context::can_yield() {
        context::preempt_enable();
        context::schedule();
        context::preempt_disable();
    }
}

#[cfg(all(any(target_os = "none", target_os = "breenix"), not(test)))]
mod context {
    #[cfg(target_arch = "x86_64")]
    use crate::per_cpu as cpu;
    #[cfg(target_arch = "aarch64")]
    use crate::per_cpu_aarch64 as cpu;

    pub fn can_yield() -> bool {
        // Foreground x86 syscalls mask IRQs; nested brakes and interrupt/boot
        // contexts also retain their caller's scheduling constraints.
        cpu::preempt_count() == 1 && !cpu::in_interrupt()
            && crate::arch_interrupts_enabled()
            && crate::task::scheduler::current_thread_id().is_some()
            // Boot runs as idle: its continuation is not a resumable worker.
            // Let the scheduler identify it on either architecture.
            && crate::task::scheduler::is_current_idle_thread() == Some(false)
    }
    pub fn need_resched() -> bool { cpu::need_resched() }
    pub fn preempt_enable() { cpu::preempt_enable(); }
    pub fn preempt_disable() { cpu::preempt_disable(); }
    pub fn schedule() {
        #[cfg(target_arch = "aarch64")]
        crate::arch_impl::aarch64::context_switch::schedule_from_kernel();
        #[cfg(target_arch = "x86_64")]
        // SAFETY: this is the existing reschedule gate used by blocking I/O.
        // It saves/restores the running continuation. scheduler::schedule on
        // x86 alone updates bookkeeping without switching the thread.
        unsafe {
            core::arch::asm!("int {vector}", vector = const crate::interrupts::RESCHEDULE_VECTOR);
        }
    }
}

// Model just the CPU context; the point and its callers are production code.
#[cfg(test)]
pub mod context {
    extern crate std;
    use core::cell::{Cell, RefCell};
    use alloc::boxed::Box;
    std::thread_local! {
        static COUNT: Cell<u32> = const { Cell::new(0) };
        static NEED: Cell<bool> = const { Cell::new(false) };
        static YIELDS: Cell<u32> = const { Cell::new(0) };
        static POINTS: Cell<u32> = const { Cell::new(0) };
        static FIRST_YIELD: Cell<u32> = const { Cell::new(0) };
    }
    // Test-only scheduler/ownership observations for E24 coordinator 597(x11a).
    // Taking the callback out prevents observer reads from recursively observing.
    #[derive(Clone, Copy)]
    pub enum Event {
        Acquire(usize), Release(usize), Access(usize), Point, Yield,
        Phase { op: &'static str, phase: &'static str, begin: bool },
    }
    type Observer = Box<dyn FnMut(Event)>;
    std::thread_local! {
        static OBSERVER: RefCell<Option<Observer>> = const { RefCell::new(None) };
    }
    pub fn observe(event: Event) {
        let callback = OBSERVER.with(|o| o.borrow_mut().take());
        if let Some(mut callback) = callback {
            callback(event);
            OBSERVER.with(|o| *o.borrow_mut() = Some(callback));
        }
    }
    pub fn observer(callback: Option<Observer>) { OBSERVER.with(|o| *o.borrow_mut() = callback); }
    pub fn request_reschedule() { NEED.with(|n| n.set(true)); }
    pub fn point() {
        POINTS.with(|n| n.set(n.get() + 1));
        observe(Event::Point);
    }
    pub fn first_yield() -> u32 { FIRST_YIELD.with(Cell::get) }
    pub fn can_yield() -> bool { COUNT.with(|n| n.get() == 1) }
    pub fn need_resched() -> bool { NEED.with(Cell::get) }
    pub fn preempt_enable() { COUNT.with(|n| n.set(n.get() - 1)); }
    pub fn preempt_disable() { COUNT.with(|n| n.set(n.get() + 1)); }
    pub fn schedule() {
        assert_eq!(count(), 0, "scheduler entered with the I/O brake held");
        observe(Event::Yield);
        FIRST_YIELD.with(|first| if first.get() == 0 { first.set(POINTS.with(Cell::get)); });
        NEED.with(|n| n.set(false));
        YIELDS.with(|n| n.set(n.get() + 1));
    }
    pub fn count() -> u32 { COUNT.with(Cell::get) }
    pub fn yields() -> u32 { YIELDS.with(Cell::get) }
    pub fn request() {
        COUNT.with(|n| n.set(1));
        NEED.with(|n| n.set(true));
        YIELDS.with(|n| n.set(0));
        POINTS.with(|n| n.set(0));
        FIRST_YIELD.with(|n| n.set(0));
    }
}
