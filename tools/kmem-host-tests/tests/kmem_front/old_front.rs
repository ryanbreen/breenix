//! The kernel heap's front at 445c4c5e, extracted unchanged from
//! kernel/src/memory/heap.rs: `IrqSafeLockedHeap::with_inner` and its
//! `GlobalAlloc::alloc`. The mutex is a parameter, interrupt masking is the
//! identity (one simulated CPU), and the suite-case counters are left out.
//! Outside a suite case `LockWait::active()` is false, so the masked branch
//! goes straight to `lock()`.
use std::alloc::Layout;
use std::ptr::NonNull;

use super::heap_index::Heap;
use super::sim::SimLock;

/// `aarch64_irqs_enabled` is the branch an ARM64 caller with interrupts
/// enabled takes; the other callers take the masked branch.
pub fn with_inner<R>(
    inner: &SimLock<Heap>,
    aarch64_irqs_enabled: bool,
    operation: impl FnOnce(&mut Heap) -> R,
) -> R {
    if aarch64_irqs_enabled {
        let mut operation = Some(operation);
        loop {
            let done = inner
                .try_lock()
                .map(|mut inner| operation.take().unwrap()(&mut inner));
            if let Some(result) = done {
                return result;
            }
            while inner.is_locked() {
                core::hint::spin_loop();
            }
        }
    }
    let mut inner = inner.lock();
    operation(&mut inner)
}

pub fn alloc(inner: &SimLock<Heap>, aarch64_irqs_enabled: bool, layout: Layout) -> *mut u8 {
    with_inner(inner, aarch64_irqs_enabled, |heap| {
        let (ptr, _walk) = heap.allocate(layout);
        ptr.map_or(core::ptr::null_mut(), NonNull::as_ptr)
    })
}
