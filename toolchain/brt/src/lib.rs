//! Atomic libcalls for objects bcc cannot access with lock-free instructions.
//!
//! The generic size-first ABI comes from LLVM 22.1.0, Atomic Instructions
//! and Concurrency Guide, “Libcalls: __atomic_*” (E25 ruling 43).
//! The GCC 15.2.0 manual §7.9.1 describes the operations and memory orders.
//! The program links one copy of this runtime, and concurrent accesses to
//! its non-lock-free objects share the locks below.

#![cfg_attr(not(test), no_std)]
#![deny(unsafe_op_in_unsafe_fn)]

pub mod f128;

#[cfg(feature = "exports")]
mod exports;

use core::ffi::{c_int, c_void};
use core::hint::spin_loop;
use core::ptr;
use core::sync::atomic::{fence, AtomicBool, Ordering};

// N1570 §7.24.4.1 (memcmp) and §7.22.4.1 (abort).
extern "C" {
    fn memcmp(left: *const c_void, right: *const c_void, size: usize) -> c_int;

    #[cfg(not(test))]
    fn abort() -> !;
}

#[cfg(not(test))]
#[panic_handler]
fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
    // SAFETY: abort takes no arguments and terminates the process.
    unsafe { abort() }
}

const BUCKET_BYTES: usize = 64;
const LOCK_COUNT: usize = u64::BITS as usize;
static LOCKS: [AtomicBool; LOCK_COUNT] = [const { AtomicBool::new(false) }; LOCK_COUNT];

#[derive(Clone, Copy)]
struct Buckets(u64);

impl Iterator for Buckets {
    type Item = usize;

    fn next(&mut self) -> Option<Self::Item> {
        if self.0 == 0 {
            return None;
        }
        let index = self.0.trailing_zeros() as usize;
        self.0 &= self.0 - 1;
        Some(index)
    }
}

fn buckets_for_span(address: usize, size: usize) -> Buckets {
    if size == 0 {
        return Buckets(0);
    }
    let first = address / BUCKET_BYTES;
    let last = address.saturating_add(size - 1) / BUCKET_BYTES;
    let covered = (last - first + 1).min(LOCK_COUNT);
    let mask = u64::MAX >> (LOCK_COUNT - covered);
    // Consecutive address ranges hash to consecutive lock indices modulo the
    // table length. Enumerate the resulting set in lock address order, even
    // when the span wraps the table or hashes to the same lock twice.
    Buckets(mask.rotate_left((first % LOCK_COUNT) as u32))
}

struct LockedSpan(Buckets);

impl LockedSpan {
    fn acquire(object: *const c_void, size: usize) -> Self {
        let buckets = buckets_for_span(object.addr(), size);
        for index in buckets {
            let lock = &LOCKS[index];
            while lock.swap(true, Ordering::Acquire) {
                while lock.load(Ordering::Relaxed) {
                    spin_loop();
                }
            }
        }
        Self(buckets)
    }
}

impl Drop for LockedSpan {
    fn drop(&mut self) {
        for index in self.0 {
            LOCKS[index].store(false, Ordering::Release);
        }
    }
}

// The lock protects object bytes even for relaxed operations. The fences
// also order surrounding accesses, including those using inline atomics.
// Consume is strengthened to acquire, as in GCC §7.9.1. The upper 16 bits
// are reserved for target modifiers. Unknown base orders use seq_cst.
fn fence_before(order: c_int) {
    match order & 0xffff {
        0..=2 => {}
        3 | 4 => fence(Ordering::Release),
        _ => fence(Ordering::SeqCst),
    }
}

fn fence_after(order: c_int) {
    match order & 0xffff {
        0 | 3 => {}
        1 | 2 | 4 => fence(Ordering::Acquire),
        _ => fence(Ordering::SeqCst),
    }
}

/// Copy the old object representation into `result`.
///
/// # Safety
/// For nonzero `size`, `object` must be live and readable for `size` bytes.
/// `result` must be disjoint from it and writable for `size` bytes. Concurrent
/// accesses to `object` must use this runtime and the same object span.
#[no_mangle]
pub unsafe extern "C" fn __atomic_load(
    size: usize,
    object: *mut c_void,
    result: *mut c_void,
    order: c_int,
) {
    if size == 0 {
        return;
    }
    let _locks = LockedSpan::acquire(object, size);
    fence_before(order);
    // SAFETY: The object is locked and the caller supplies disjoint buffers.
    unsafe { ptr::copy_nonoverlapping(object.cast::<u8>(), result.cast::<u8>(), size) };
    fence_after(order);
}

/// Replace the object representation with a copy of `value`.
///
/// # Safety
/// For nonzero `size`, `object` must be live and writable for `size` bytes.
/// `value` must be disjoint from it and readable for `size` bytes. Concurrent
/// accesses to `object` must use this runtime and the same object span.
#[no_mangle]
pub unsafe extern "C" fn __atomic_store(
    size: usize,
    object: *mut c_void,
    value: *mut c_void,
    order: c_int,
) {
    if size == 0 {
        return;
    }
    let _locks = LockedSpan::acquire(object, size);
    fence_before(order);
    // SAFETY: The object is locked and the caller supplies disjoint buffers.
    unsafe { ptr::copy_nonoverlapping(value.cast::<u8>(), object.cast::<u8>(), size) };
    fence_after(order);
}

/// Copy the old representation into `result` and replace it with `value`.
///
/// # Safety
/// For nonzero `size`, the three buffers must be live and disjoint for `size`
/// bytes. `object` must be readable and writable, `value` readable and
/// `result` writable. Concurrent accesses to `object` must use this runtime
/// and the same object span.
#[no_mangle]
pub unsafe extern "C" fn __atomic_exchange(
    size: usize,
    object: *mut c_void,
    value: *mut c_void,
    result: *mut c_void,
    order: c_int,
) {
    if size == 0 {
        return;
    }
    let _locks = LockedSpan::acquire(object, size);
    fence_before(order);
    // SAFETY: The object is locked and result is a disjoint output buffer.
    unsafe { ptr::copy_nonoverlapping(object.cast::<u8>(), result.cast::<u8>(), size) };
    // SAFETY: The object is locked and value is a disjoint input buffer.
    unsafe { ptr::copy_nonoverlapping(value.cast::<u8>(), object.cast::<u8>(), size) };
    fence_after(order);
}

/// Replace a matching representation, otherwise update `expected` in place.
///
/// # Safety
/// For nonzero `size`, the three buffers must be live for `size` bytes.
/// `object` must be readable and writable, and disjoint from both input buffers.
/// `expected` must be readable and writable, and `desired` readable.
/// Concurrent accesses to `object` must use this runtime and the same span.
#[no_mangle]
pub unsafe extern "C" fn __atomic_compare_exchange(
    size: usize,
    object: *mut c_void,
    expected: *mut c_void,
    desired: *mut c_void,
    success_order: c_int,
    failure_order: c_int,
) -> bool {
    if size == 0 {
        return true;
    }
    let _locks = LockedSpan::acquire(object, size);
    // Failure cannot request a stronger order than success. Its trailing
    // fence still uses the failure order when the comparison fails.
    fence_before(success_order);
    // SAFETY: The object is locked and expected is readable. memcmp compares
    // their byte representations without creating typed Rust references.
    let matched = unsafe { memcmp(object, expected, size) } == 0;
    if matched {
        // SAFETY: The object is locked and desired is a disjoint input buffer.
        unsafe { ptr::copy_nonoverlapping(desired.cast::<u8>(), object.cast::<u8>(), size) };
        fence_after(success_order);
    } else {
        // SAFETY: The object is locked and expected is a disjoint output buffer.
        unsafe { ptr::copy_nonoverlapping(object.cast::<u8>(), expected.cast::<u8>(), size) };
        fence_after(failure_order);
    }
    matched
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod binary128_tests;

#[cfg(all(test, feature = "exports", target_arch = "aarch64"))]
mod fpcr_tests;
