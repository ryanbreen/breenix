//! One suite request at a time; only the owning xHCI worker touches its controller.
use crate::{input::control::Receipt, syscall::errno::*};
use core::sync::atomic::{AtomicBool, Ordering};
#[derive(Clone, Copy)]
pub(super) enum Request { Keyboard, Pointer(i32, i32) }
static REQUESTED: AtomicBool = AtomicBool::new(false);
static BUSY: AtomicBool = AtomicBool::new(false);
static PENDING: spin::Mutex<Option<(Request, bool)>> = spin::Mutex::new(None);
static ANSWER: spin::Mutex<Option<Result<Receipt, u64>>> = spin::Mutex::new(None);

pub(super) fn pending() -> bool { REQUESTED.load(Ordering::Acquire) }
pub(super) fn take() -> Option<(Request, bool)> {
    if !REQUESTED.swap(false, Ordering::AcqRel) { return None; }
    PENDING.lock().take()
}
pub(super) fn answer(result: Result<Receipt, u64>) { *ANSWER.lock() = Some(result); }
pub(super) fn submit(request: Request, suppress: bool) -> Result<Receipt, u64> {
    if !crate::test_control::enabled() { return Err(ENOSYS as u64); }
    if BUSY.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).is_err() {
        return Err(EBUSY as u64);
    }
    *ANSWER.lock() = None;
    *PENDING.lock() = Some((request, suppress));
    REQUESTED.store(true, Ordering::Release);
    // request() supplies the existing publish/check/wake ordering.
    super::request(&super::LEDS);
    // Do not abandon an in-flight DMA transaction on a wall-clock timeout.
    // Kernel command waits are bounded; the worker owns completion through requeue.
    loop {
        if let Some(result) = ANSWER.lock().take() {
            BUSY.store(false, Ordering::Release);
            return result;
        }
        if super::FAILED.load(Ordering::Acquire) {
            REQUESTED.store(false, Ordering::Release);
            PENDING.lock().take();
            BUSY.store(false, Ordering::Release);
            return Err(EIO as u64);
        }
        crate::task::scheduler::yield_current();
    }
}
