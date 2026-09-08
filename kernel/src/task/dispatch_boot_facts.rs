//! Deferred x86 boot-stage markers. The IRQ-return path records selection and
//! a saved CS with RPL=3; the thread-context reporter emits the text with IF=1.

use core::sync::atomic::{AtomicU8, Ordering};

#[derive(Clone, Copy)]
#[repr(u8)]
pub(crate) enum BootDispatchFact {
    ScheduleReturned = 1,
    UserspaceReturned = 2,
}

static SEEN: AtomicU8 = AtomicU8::new(0);
static EMITTED: AtomicU8 = AtomicU8::new(0);

pub(crate) fn note(fact: BootDispatchFact) {
    let bit = fact as u8;
    if SEEN.load(Ordering::Relaxed) & bit == 0 {
        SEEN.fetch_or(bit, Ordering::Release);
    }
}

pub(crate) fn emit_if_enabled() {
    if !crate::arch_interrupts_enabled() {
        return;
    }
    let mut seen = SEEN.load(Ordering::Acquire);
    // CS.RPL=3 establishes a userspace return; require the syscall-side latch
    // as well before emitting the existing smoke marker's syscall claim.
    if !crate::syscall::handler::is_ring3_confirmed() {
        seen &= !(BootDispatchFact::UserspaceReturned as u8);
    }
    let fresh = seen & !EMITTED.fetch_or(seen, Ordering::Relaxed);
    if fresh & BootDispatchFact::ScheduleReturned as u8 != 0 {
        crate::serial_println!("[ INFO] scheduler::schedule() returned (boot marker)");
    }
    if fresh & BootDispatchFact::UserspaceReturned as u8 != 0 {
        crate::serial_println!("RING3_ENTER: CS=0x33");
        crate::serial_println!("[ OK ] RING3_SMOKE: userspace executed + syscall path verified");
    }
}
