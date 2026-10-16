//! Run with rustc --edition 2021 --test tests/gpu_clean_room.rs.
//! These execute the driver's allocation and wire helpers on the host.
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

#[path = "../kernel/src/drivers/virtio/command_dma.rs"]
mod command_dma;

struct TrackingAllocator;
static WATCHED: AtomicUsize = AtomicUsize::new(0);
static RELEASED: AtomicBool = AtomicBool::new(false);
unsafe impl GlobalAlloc for TrackingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 { System.alloc(layout) }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 { System.alloc_zeroed(layout) }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if ptr as usize == WATCHED.load(Ordering::SeqCst) { RELEASED.store(true, Ordering::SeqCst); }
        System.dealloc(ptr, layout);
    }
}
#[global_allocator]
static ALLOCATOR: TrackingAllocator = TrackingAllocator;

#[test]
fn request_storage_survives_device_ownership_and_releases_only_after_return() {
    use command_dma::DmaRequest;
    let mut command = [1u8; 32];
    let payload = [2u8; 7];
    let mut request = DmaRequest::new(&command, &payload, 24).unwrap();
    command.fill(3);
    assert_eq!(request.ptr(0) as usize % 4096, 0);
    assert_eq!(request.ptr(1) as usize - request.ptr(0) as usize, 64);
    assert_eq!(request.ptr(2) as usize - request.ptr(1) as usize, 64);
    assert_eq!([request.len(0), request.len(1), request.len(2)], [32, 7, 24]);
    unsafe {
        assert_eq!(std::slice::from_raw_parts(request.ptr(0), 32), &[1; 32]);
        assert_eq!(std::slice::from_raw_parts(request.ptr(1), 7), &payload);
        assert_eq!(std::slice::from_raw_parts(request.ptr(2), 24), &[0xa5; 24]);
    }
    WATCHED.store(request.ptr(0) as usize, Ordering::SeqCst);
    RELEASED.store(false, Ordering::SeqCst);
    request.publish();
    // A timeout drops the local handle, without returning ownership.
    let quarantined = request.ptr(0);
    drop(request);
    assert!(!RELEASED.load(Ordering::SeqCst));
    unsafe {
        assert_eq!(*quarantined, 1);
        // Test teardown after simulating a device reset; production never
        // reclaims a quarantined allocation without a completed reset.
        System.dealloc(quarantined, Layout::from_size_align(192, 4096).unwrap());
    }
    let mut request = DmaRequest::new(&command, &[], 24).unwrap();
    WATCHED.store(request.ptr(0) as usize, Ordering::SeqCst);
    RELEASED.store(false, Ordering::SeqCst);
    request.publish();
    request.returned();
    drop(request);
    assert!(RELEASED.load(Ordering::SeqCst));
    WATCHED.store(0, Ordering::SeqCst);
}
