//! Host checks for read(2) on a regular file (#3040, E24 ledger 379).
//!
//! The kernel copies a regular file's bytes to the caller through one bounded
//! buffer in a loop (`kernel/src/syscall/regular_read.rs`). These tests run
//! that loop on the host with a test backend and a test user buffer, under an
//! allocator that refuses large blocks the way a guest kernel heap does.
#![cfg(test)]

extern crate alloc;

#[path = "../../../kernel/src/syscall/regular_read.rs"]
mod regular_read;

use regular_read::{read, Failure, CHUNK};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

thread_local! {
    /// While set, allocations on this thread above this size fail.
    static REFUSE_ABOVE: Cell<Option<usize>> = const { Cell::new(None) };
    /// The largest allocation requested on this thread while armed.
    static LARGEST: Cell<usize> = const { Cell::new(0) };
}

struct Heap;

unsafe impl GlobalAlloc for Heap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let refuse = REFUSE_ABOVE.try_with(|limit| limit.get()).ok().flatten();
        if let Some(limit) = refuse {
            let _ = LARGEST.try_with(|largest| largest.set(largest.get().max(layout.size())));
            if layout.size() > limit {
                return std::ptr::null_mut();
            }
        }
        System.alloc(layout)
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout)
    }
}

#[global_allocator]
static HEAP: Heap = Heap;

/// Runs `body` with allocations above `limit` refused; returns its result and
/// the largest allocation it asked for.
fn with_heap_limit<T>(limit: usize, body: impl FnOnce() -> T) -> (T, usize) {
    LARGEST.with(|largest| largest.set(0));
    REFUSE_ABOVE.with(|refuse| refuse.set(Some(limit)));
    let result = body();
    REFUSE_ABOVE.with(|refuse| refuse.set(None));
    (result, LARGEST.with(|largest| largest.get()))
}

/// File contents: a period (251) that does not divide any chunk size, so a
/// byte delivered at the wrong offset is caught.
fn byte_at(offset: u64) -> u8 {
    (offset % 251) as u8
}

/// A backend holding `size` bytes of `byte_at` content.
fn file(size: u64) -> impl FnMut(u64, &mut [u8]) -> Result<usize, i32> {
    move |offset, buf| {
        let available = size.saturating_sub(offset).min(buf.len() as u64) as usize;
        for (i, slot) in buf[..available].iter_mut().enumerate() {
            *slot = byte_at(offset + i as u64);
        }
        Ok(available)
    }
}

#[test]
fn large_read_needs_no_allocation_the_size_of_the_request() {
    // The fill case's observer reads a 48 MiB file in one read(2); a kernel
    // heap that cannot hand out a block that large must still serve it.
    const SIZE: usize = 48 * 1024 * 1024;
    let position = 4096u64;
    let mut delivered = 0usize;
    let (result, largest) = with_heap_limit(1024 * 1024, || {
        read(position, SIZE, CHUNK, file(position + SIZE as u64), |at, bytes| {
            assert_eq!(at, delivered, "chunks reach the user buffer in order");
            assert!(bytes
                .iter()
                .enumerate()
                .all(|(i, byte)| *byte == byte_at(position + (at + i) as u64)));
            delivered += bytes.len();
            Ok(())
        })
    });
    assert_eq!(result, Ok(SIZE));
    assert_eq!(delivered, SIZE);
    assert!(largest <= CHUNK, "largest kernel allocation {largest} exceeds {CHUNK}");
}

#[test]
fn short_read_eof_and_offsets() {
    // The file holds 9000 bytes past the offset but the caller asked for
    // 20000 (a truncation after the size check): a short read, no more calls.
    let mut fills = Vec::new();
    let mut deliveries = Vec::new();
    let mut backend = file(1000 + 9000);
    let result = read(1000, 20_000, 4096, |offset, buf: &mut [u8]| {
        fills.push((offset, buf.len()));
        backend(offset, buf)
    }, |at, bytes| {
        deliveries.push((at, bytes.len()));
        Ok(())
    });
    assert_eq!(result, Ok(9000));
    assert_eq!(fills, [(1000, 4096), (5096, 4096), (9192, 4096)]);
    assert_eq!(deliveries, [(0, 4096), (4096, 4096), (8192, 808)]);

    // A request that ends on a chunk boundary makes no extra call.
    let mut calls = 0;
    let mut backend = file(1 << 20);
    assert_eq!(read(0, 8192, 4096, |offset, buf: &mut [u8]| {
        calls += 1;
        backend(offset, buf)
    }, |_, _| Ok(())), Ok(8192));
    assert_eq!(calls, 2);

    // At end of file the backend is still asked once and the copy is skipped,
    // so an unwritable buffer reads 0 rather than EFAULT.
    let mut asked = Vec::new();
    assert_eq!(read(77, 0, 4096, |offset, buf: &mut [u8]| {
        asked.push((offset, buf.len()));
        Ok::<usize, i32>(0)
    }, |_, _| Err(())), Ok(0));
    assert_eq!(asked, [(77, 0)]);
}

#[test]
fn faults_and_backend_errors() {
    let size = 1u64 << 20;
    // An unwritable user buffer is EFAULT wherever it fails; the caller then
    // leaves the file offset unchanged, as it did for a single copy.
    for failing in [0usize, 4096] {
        let result = read(0, 3 * 4096, 4096, file(size), |at, _| {
            if at == failing { Err(()) } else { Ok(()) }
        });
        assert_eq!(result, Err(Failure::Fault));
    }
    // A backend error before any byte moves is the call's error.
    assert_eq!(read(0, 3 * 4096, 4096, |_, _| Err(5), |_, _| Ok(())), Err(Failure::Backend(5)));
    // After progress the bytes already delivered are the result; the error
    // is reported by the next read, at the offset where it occurs.
    let mut backend = file(size);
    let mut calls = 0;
    let result = read(0, 3 * 4096, 4096, |offset, buf: &mut [u8]| {
        calls += 1;
        if offset == 4096 { Err(5) } else { backend(offset, buf) }
    }, |_, _| Ok(()));
    assert_eq!(result, Ok(4096));
    assert_eq!(calls, 2);
}

#[test]
fn no_buffer_is_enomem() {
    let mut called = false;
    let (result, _) = with_heap_limit(1024, || {
        read(0, 1 << 20, 4096, |_, _| {
            called = true;
            Ok::<usize, i32>(0)
        }, |_, _| Ok(()))
    });
    assert_eq!(result, Err(Failure::NoMemory));
    assert!(!called);
}
