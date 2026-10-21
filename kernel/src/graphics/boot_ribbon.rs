//! Producer-observed boot stages, preserved independently of display ownership.
//! UART ownership serializes bytes; updates use atomics and never draw on the writer path.
use core::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};
const MARKERS: &[&str] = include!(concat!(env!("OUT_DIR"), "/boot_ribbon.rs"));
static REACHED: [AtomicBool; MARKERS.len()] = [const { AtomicBool::new(false) }; MARKERS.len()];
// UART ownership gives one producer. Preserve the serial bytes independently of REACHED;
// matching and rendering happen only in thread context with IRQs enabled.
const CAPACITY: usize = 512 * 1024;
static SERIAL: [AtomicU8; CAPACITY] = [const { AtomicU8::new(0) }; CAPACITY];
static TAIL: AtomicUsize = AtomicUsize::new(0);
static DIRTY: AtomicBool = AtomicBool::new(false);
static OVERFLOW: AtomicBool = AtomicBool::new(false);
static SCAN: spin::Mutex<(usize, [u8; 1024], usize)> = spin::Mutex::new((0, [0; 1024], 0));
/// Constant work per byte under UART ownership, including before the framebuffer exists.
pub fn byte(b: u8) {
    let tail = TAIL.load(Ordering::Relaxed);
    if tail == CAPACITY { OVERFLOW.store(true, Ordering::Release); return; }
    SERIAL[tail].store(b, Ordering::Relaxed);
    TAIL.store(tail + 1, Ordering::Release);
}
/// Drain in thread context; a bounded byte batch avoids monopolizing the render thread.
pub fn drain() -> bool {
    if !crate::arch_interrupts_enabled() { return false; }
    let Some(mut scan) = SCAN.try_lock() else { return false; };
    let end = TAIL.load(Ordering::Acquire).min(scan.0 + 8192);
    let mut changed = false;
    while scan.0 < end {
        let b = SERIAL[scan.0].load(Ordering::Relaxed);
        scan.0 += 1;
        let n = scan.2;
        if b == b'\n' {
            if n <= 1024 {
                let bytes = &scan.1[..n];
                for (i, marker) in MARKERS.iter().enumerate() {
                    if super::boot_producer::matches(bytes, marker, MARKERS) {
                        changed |= !REACHED[i].swap(true, Ordering::AcqRel);
                    }
                }
            }
            scan.2 = 0;
        } else {
            if n < 1024 { scan.1[n] = b; }
            scan.2 = n.saturating_add(1);
        }
    }
    if changed { DIRTY.store(true, Ordering::Release); }
    changed
}
pub fn take_dirty() -> bool { DIRTY.swap(false, Ordering::AcqRel) }
/// Complete serial prefix, not the matcher's interpretation of it. Never silently truncate.
pub fn serial() -> Result<alloc::string::String, i32> {
    if OVERFLOW.load(Ordering::Acquire) { return Err(-75); }
    let n = TAIL.load(Ordering::Acquire);
    let bytes: alloc::vec::Vec<u8> = SERIAL[..n].iter().map(|b| b.load(Ordering::Relaxed)).collect();
    Ok(alloc::string::String::from_utf8_lossy(&bytes).into_owned())
}
pub fn reached() -> alloc::vec::Vec<bool> {
    drain();
    REACHED[..MARKERS.len()]
        .iter()
        .map(|v| v.load(Ordering::Acquire))
        .collect()
}
pub fn records() -> alloc::string::String {
    drain();
    use core::fmt::Write;
    let mut out = alloc::string::String::new();
    for (i, m) in MARKERS.iter().enumerate() {
        let _ = writeln!(
            out,
            "{}|{}",
            u8::from(REACHED[i].load(Ordering::Acquire)),
            m
        );
    }
    out
}
// TODO(gpu-display): `draw` lost its callers when main's boot screen and console replaced the
// software compositor's; it moves onto `console::splash` and the GPU compositor's display.
pub fn draw(pixels: &mut [u8]) {
    let Ok(mut fb) = (unsafe {
        libgfx_ref::framebuf::FrameBuf::from_raw(pixels.as_mut_ptr(), 1280, 800, 5120, 4, true)
    }) else {
        return;
    };
    let mut stages = reached();
    if crate::test_control::take(breengel_abi::tc_point::RIBBON_DROP_STAGE, 0).is_some() {
        if let Some(s) = stages.iter_mut().find(|s| **s) {
            *s = false;
        }
    }
    let _ = libgfx_ref::stage::draw_boot_ribbon(&mut fb, &stages);
}
