//! Run retained production buffers for the case's 60 + 600 epochs, without a GPU.
include!("fixtures/bwm_memory_runtime.rs");
#[allow(dead_code)] // The path-included modules also implement rendering and input policy outside this workload.
#[path = "../userspace/programs/src/bwm/compositor.rs"]
mod compositor;
#[allow(dead_code)] // Other queue operations are exercised in bwm_validation.rs.
#[path = "../userspace/programs/src/bwm/chrome/validation.rs"]
mod validation;
#[allow(dead_code)]
#[path = "../userspace/programs/src/bwm/timing.rs"]
mod timing;

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::{Mutex, atomic::{AtomicBool, AtomicUsize, Ordering}};
static EPOCH: AtomicUsize = AtomicUsize::new(0);
static TRACK: AtomicBool = AtomicBool::new(false);
// No allocation by the observer itself; only one test runs at a time.
static LIVE: Mutex<[(usize, usize, usize); 4096]> = Mutex::new([(0, 0, 0); 4096]);
struct Observe;
unsafe impl GlobalAlloc for Observe {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() && TRACK.load(Ordering::Relaxed) {
            let mut live = LIVE.lock().unwrap();
            let slot = live.iter_mut().find(|s| s.0 == 0).expect("observer slots");
            *slot = (p as usize, layout.size(), EPOCH.load(Ordering::Relaxed));
        }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
        let mut live = LIVE.lock().unwrap();
        if let Some(slot) = live.iter_mut().find(|s| s.0 == p as usize) { *slot = (0, 0, 0); }
        drop(live);
        unsafe { System.dealloc(p, layout); }
    }
}
#[global_allocator] static ALLOCATOR: Observe = Observe;

fn retained_capacity_after_warmup() -> usize {
    LIVE.lock().unwrap().iter().filter(|s| s.2 > 60).map(|s| s.1).sum()
}
fn epochs(mut step: impl FnMut(usize)) {
    let mut replacement_capacity = 0;
    for epoch in 1..=660 {
        EPOCH.store(epoch, Ordering::Relaxed);
        TRACK.store(true, Ordering::Relaxed);
        step(epoch);
        TRACK.store(false, Ordering::Relaxed);
        if epoch > 60 { replacement_capacity += retained_capacity_after_warmup(); }
    }
    assert_eq!(replacement_capacity, 0, "retained capacity was allocated after epoch 60");
}

#[test]
fn sampled_eight_window_frames_reuse_retained_capacity() {
    let mut gpu = compositor::Compositor::open(800, 600, 36, 1).unwrap_or_else(|e| panic!("{e}"));
    let layers: Vec<_> = (1..=8).map(|id| compositor::Layer {
        window_id: id, frame: [[0; 4]; 3], frame_len: 0, content: [0, 0, 32, 32],
    }).collect();
    epochs(|epoch| {
        gpu.compose(&layers, None, &[[0, 0, 800, 600]]).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(gpu.last_serial, epoch as u64);
    });
}

#[test]
fn latest_validation_scene_reuses_eight_layers() {
    let mut queue = validation::ValidationQueue::new();
    epochs(|epoch| {
        queue.presented((0..8).map(|id| [id, epoch as i32, 32, 32]).collect::<Vec<_>>());
        assert_eq!(queue.scene().unwrap()[7], [7, epoch as i32, 32, 32]);
    });
}

#[test]
fn removed_pointer_devices_do_not_accumulate_click_history() {
    let mut clicks = timing::Clicks::new();
    epochs(|epoch| {
        // Eight windows, successive short-lived pointer devices, beyond the
        // existing double-click interval. Each release must keep its press count.
        for window in 1..=8 {
            let device = (epoch * 8 + window) as u32;
            assert_eq!(clicks.press(device, 0x110, window as u64, (0, 0), epoch as u64 * 500_000_000), 1);
            assert_eq!(clicks.release(device, 0x110), 1);
        }
    });
}
