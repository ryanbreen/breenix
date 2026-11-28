//! E3-1: the readahead window (plan sections 8.1 and 8.2).
use std::collections::BTreeSet;

use pagecache_host_tests::readahead::{
    max_window, windows, Hint, Readahead, Window, CAP_PAGES, NO_TRIGGER,
};
use pagecache_host_tests::Lcg;

const MAX: u32 = 512;
const GIB_PAGES: u64 = 262_144;

/// A reader of one page at a time over `indices`: a miss asks for a window,
/// and a hit on the trigger page queues the next one. Returns the windows in
/// the order they were issued, which is one device request each.
fn read(ra: &mut Readahead, indices: impl IntoIterator<Item = u64>, request: u64) -> Vec<(u64, u32)> {
    let mut resident = BTreeSet::new();
    let mut issued = Vec::new();
    let mut issue = |w: Window, resident: &mut BTreeSet<u64>| {
        assert!(w.size <= MAX, "window {w:?} passes max");
        issued.push((w.start, w.size));
        resident.extend(w.start..w.end());
    };
    for index in indices {
        if !resident.contains(&index) {
            let w = ra.on_miss(index, request, MAX);
            issue(w, &mut resident);
        }
        if let Some(w) = ra.on_async_hit(index, MAX) {
            issue(w, &mut resident);
        }
    }
    issued
}

#[test]
fn every_path_is_capped_at_max_including_a_one_gib_request() {
    let first = Readahead::new().on_miss(0, GIB_PAGES, MAX);
    assert_eq!((first.start, first.size, first.trigger), (0, MAX, NO_TRIGGER));

    let mut reset = Readahead::new();
    reset.on_miss(1_000_000, 8, MAX);
    let w = reset.on_miss(5, GIB_PAGES, MAX);
    assert_eq!((w.start, w.size), (5, MAX));

    for hint in [Hint::Sequential, Hint::Random, Hint::Normal] {
        let mut ra = Readahead::new();
        ra.set_hint(hint);
        let w = ra.on_miss(77, GIB_PAGES, MAX);
        assert_eq!(w.size, MAX, "{hint:?}");
        // A demanded request larger than max is filled in successive windows
        // of at most max, each sequential after the last.
        let mut next = w.end();
        for _ in 0..4 {
            let w = ra.on_miss(next, GIB_PAGES, MAX);
            assert_eq!((w.start, w.size), (next, MAX), "{hint:?}");
            next = w.end();
        }
    }
    let mut seq = Readahead::new();
    seq.set_hint(Hint::Sequential);
    assert_eq!(seq.on_miss(9, 1, MAX).size, MAX);

    let parts: Vec<_> = windows(3, GIB_PAGES, MAX).collect();
    assert_eq!(parts.len() as u64, GIB_PAGES / u64::from(MAX));
    let mut expect = 3;
    for w in &parts {
        assert!(w.size <= MAX);
        assert_eq!(w.start, expect, "windows leave no gap");
        expect = w.end();
    }
    assert_eq!(expect, 3 + GIB_PAGES);

    // max: transfer times depth, capped, halved under pressure, at least 1.
    assert_eq!(max_window(16, 1, false), 16);
    assert_eq!(max_window(256, 64, false), CAP_PAGES);
    assert_eq!(max_window(256, 64, true), CAP_PAGES / 2);
    assert_eq!(max_window(1, 1, true), 1);
}

#[test]
fn a_sequential_stream_doubles_its_window_up_to_max() {
    let issued = read(&mut Readahead::new(), 0..4096, 1);
    let mut expected = Vec::new();
    let (mut start, mut size) = (0u64, 2u32);
    // Readahead stays one window ahead: the hit on the last window's first
    // page queues one more.
    loop {
        expected.push((start, size));
        if start >= 4096 {
            break;
        }
        start += u64::from(size);
        size = (size * 2).min(MAX);
    }
    assert_eq!(issued, expected);
    // 2, 4, ..., 512 is 9 windows; then 512s up to one past page 4095.
    assert_eq!(issued.len(), 17);
}

#[test]
fn a_random_stream_seed_5eed_never_grows() {
    let mut rng = Lcg(0x5eed);
    let indices: Vec<u64> = (0..200).map(|_| 1 + rng.below(1 << 30)).collect();
    let issued = read(&mut Readahead::new(), indices.iter().copied(), 1);
    assert_eq!(issued.len(), indices.len(), "one request per random read");
    assert!(issued.iter().all(|&(_, size)| size == 4), "{issued:?}");

    let mut random = Readahead::new();
    random.set_hint(Hint::Random);
    let issued = read(&mut random, indices.iter().copied(), 1);
    assert!(issued.iter().all(|&(_, size)| size == 1));
}

#[test]
fn interleaved_streams_grow_only_with_their_own_state() {
    // Two descriptions, each with its own state: both grow.
    let (mut a, mut b) = (Readahead::new(), Readahead::new());
    let mut sizes = (Vec::new(), Vec::new());
    for step in 0..6u64 {
        let wa = a.on_miss(1000 + 4 * ((1 << step) - 1), 4, MAX);
        let wb = b.on_miss(900_000 + 4 * ((1 << step) - 1), 4, MAX);
        sizes.0.push(wa.size);
        sizes.1.push(wb.size);
    }
    assert_eq!(sizes.0, vec![4, 8, 16, 32, 64, 128]);
    assert_eq!(sizes.1, sizes.0);

    // One state shared by two interleaved streams sees no sequence.
    let mut shared = Readahead::new();
    for step in 0..6u64 {
        assert_eq!(shared.on_miss(1000 + 4 * step, 4, MAX).size, 4, "step {step}");
        assert_eq!(shared.on_miss(900_000 + 4 * step, 4, MAX).size, 4, "step {step}");
    }
}

#[test]
fn a_backward_step_resets_the_window() {
    let mut ra = Readahead::new();
    let issued = read(&mut ra, 0..200, 1);
    let last = *issued.last().unwrap();
    assert_eq!(last, (254, 256));
    // Back to a page the stream left behind, outside the current window.
    let w = ra.on_miss(10, 1, MAX);
    assert_eq!((w.start, w.size, w.trigger), (10, 4, 11));
    // And forward from there it grows again.
    let w = ra.on_miss(11, 1, MAX);
    assert_eq!(w.size, 8);
}
