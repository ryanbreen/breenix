//! Run the kernel's contiguous reservation algorithm against fragmented maps.
#[path = "../kernel/src/memory/contiguous.rs"]
mod contiguous;

use std::cell::RefCell;
use std::sync::atomic::{AtomicUsize, Ordering};

#[test]
fn sixteen_isolated_pages_do_not_hide_a_later_dma_run() {
    let mut pages: Vec<u64> = (0..16).map(|i| 0x1000 + i * 0x2000).collect();
    pages.extend([0x100000, 0x101000, 0x102000]);
    let frontier = AtomicUsize::new(0);
    let released = RefCell::new(Vec::new());
    let mut out = [None; 2];
    assert_eq!(
        contiguous::allocate(
            2, &mut out, &frontier, |i| pages.get(i).copied(), |f| f,
            |_, _| true, |_| true, |f| released.borrow_mut().push(f),
        ),
        Some(0x100000)
    );
    assert_eq!(out, [Some(0x100000), Some(0x101000)]);
    assert_eq!(*released.borrow(), pages[..16]);
    assert_eq!(frontier.load(Ordering::SeqCst), 18);
}

#[test]
fn exhaustion_returns_partial_claims_without_a_noncontiguous_result() {
    let pages = [0x1000, 0x3000, 0x5000];
    let released = RefCell::new(Vec::new());
    let mut out = [None; 2];
    assert_eq!(
        contiguous::allocate(
            2, &mut out, &AtomicUsize::new(0), |i| pages.get(i).copied(), |f| f,
            |_, _| true, |_| true, |f| released.borrow_mut().push(f),
        ),
        None
    );
    assert_eq!(out, [None; 2]);
    assert_eq!(*released.borrow(), pages);
}

#[test]
fn frontier_contention_remeasures_before_claiming() {
    let pages = [0x1000, 0x2000, 0x3000];
    let frontier = AtomicUsize::new(0);
    let mut out = [None; 2];
    assert_eq!(
        contiguous::allocate(
            2, &mut out, &frontier, |i| pages.get(i).copied(), |f| f,
            |base, _| { if base == 0 { frontier.store(1, Ordering::SeqCst); } true },
            |_| true, |_| panic!("a complete run must not be released"),
        ),
        Some(0x2000)
    );
    assert_eq!(out, [Some(0x2000), Some(0x3000)]);
}
