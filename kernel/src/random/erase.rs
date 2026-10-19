//! Explicit erasure for secret-bearing byte and word buffers.

/// Volatile stores cannot be removed as dead stores when a buffer goes out of
/// scope. The fence keeps surrounding accesses on their side of the erasure.
#[inline(never)]
pub fn wipe<T: Copy + Default>(buffer: &mut [T]) {
    for item in buffer {
        // SAFETY: item is an exclusive reference to a live, aligned element.
        unsafe { core::ptr::write_volatile(item, T::default()) };
    }
    core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
}
