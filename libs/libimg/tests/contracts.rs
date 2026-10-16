use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
thread_local! { static REFUSE_SIZE: Cell<usize> = const { Cell::new(usize::MAX) }; }
struct FailingAllocator;
unsafe impl GlobalAlloc for FailingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if refuse(layout.size()) { std::ptr::null_mut() } else { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        if refuse(layout.size()) { std::ptr::null_mut() } else { System.alloc_zeroed(layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        if refuse(size) { std::ptr::null_mut() } else { System.realloc(ptr, layout, size) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) { System.dealloc(ptr, layout); }
}
fn refuse(size: usize) -> bool {
    REFUSE_SIZE.try_with(|wanted| {
        if wanted.get() != size { return false; }
        wanted.set(usize::MAX);
        true
    }).unwrap_or(false)
}
#[global_allocator]
static ALLOCATOR: FailingAllocator = FailingAllocator;
mod checks;
static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
fn run(prefix: &str) {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut failures = Vec::new();
    for &(name, run) in checks::CHECKS {
        let _ = checks::take_judged_images();
        if name.starts_with(prefix) {
            match std::panic::catch_unwind(run) {
                Ok(Ok(())) => {}
                Ok(Err(e)) => failures.push(format!("{name}: {e}")),
                Err(_) => failures.push(format!("{name}: codec panicked")),
            }
        }
        for frame in checks::take_judged_images() {
            assert!(!frame.label.is_empty());
            for image in [frame.actual, frame.expected] {
                assert_eq!(image.pixels.len(), image.width as usize * image.height as usize * 4);
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
#[test]
fn inflate_contracts() {
    run("inflate_");
}
#[test]
fn png_decode_contracts() {
    run("png_decode_");
}
#[test]
fn png_encode_contracts() {
    run("png_encode_");
}
#[test]
fn bmp_contracts() {
    run("bmp_");
}

#[test]
fn allocation_errors_return_without_aborting() {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    REFUSE_SIZE.with(|size| size.set(4096));
    let result = libimg::inflate::inflate(&[1, 1, 0, 254, 255, 42], usize::MAX);
    REFUSE_SIZE.with(|size| size.set(usize::MAX));
    assert_eq!(result, Err(libimg::inflate::InflateError::OutOfMemory));
    let png = checks::pngfile(&checks::ihdr(1, 1, 8, 0, 0), &[], &[0, 42]);
    REFUSE_SIZE.with(|size| size.set(4));
    let result = libimg::png::decode(&png);
    REFUSE_SIZE.with(|size| size.set(usize::MAX));
    assert_eq!(result, Err(libimg::png::PngError::OutOfMemory));
}

#[test]
fn judged_images_retain_alpha_and_fixture_sequence() {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _ = checks::take_judged_images();
    checks::png_decode_trns_gray().unwrap();
    let images = checks::take_judged_images();
    assert_eq!(images.len(), 9); // five depths, plus four keys with high bits
    assert_eq!(&images[1].actual.pixels[..4], &[0, 0, 0, 0]);
    assert_eq!(&images[1].expected.pixels[..4], &[0, 0, 0, 0]);
    checks::png_decode_independent_fixtures().unwrap();
    let images = checks::take_judged_images();
    assert_eq!(images.len(), 72);
    assert_ne!(images[0].label, images[71].label);
}
