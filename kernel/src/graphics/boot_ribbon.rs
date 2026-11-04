//! Producer-observed boot stages, preserved independently of display ownership.
//! UART ownership serializes bytes; updates use atomics and never draw on the writer path.
use core::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, AtomicUsize, Ordering};
const MARKERS: &[&str] = include!(concat!(env!("OUT_DIR"), "/boot_ribbon.rs"));
static REACHED: [AtomicBool; MARKERS.len()] = [const { AtomicBool::new(false) }; MARKERS.len()];
// UART ownership gives one producer. Preserve the serial bytes independently of REACHED;
// matching and rendering happen only in thread context with IRQs enabled.
const CAPACITY: usize = 512 * 1024;
static SERIAL: [AtomicU8; CAPACITY] = [const { AtomicU8::new(0) }; CAPACITY];
// Indexed by the newline byte's offset, published with TAIL. This costs 4 MiB
// of zero-initialized storage and keeps writer work bounded, without locks or
// parsing, even when the display owner prevents the console thread from drawing.
static LINE_MS: [AtomicU64; CAPACITY] = [const { AtomicU64::new(0) }; CAPACITY];
static TAIL: AtomicUsize = AtomicUsize::new(0);
static OVERFLOW: AtomicBool = AtomicBool::new(false);
type Scan = (usize, [u8; 1024], usize, crate::console::status::SuiteStatus);
static SCAN: spin::Mutex<Scan> = spin::Mutex::new((0, [0; 1024], 0, crate::console::status::SuiteStatus::new()));
/// Constant work per byte under UART ownership, including before the framebuffer exists.
pub fn byte(b: u8) {
    let tail = TAIL.load(Ordering::Relaxed);
    let Some(slot) = SERIAL.get(tail) else { OVERFLOW.store(true, Ordering::Release); return; };
    slot.store(b, Ordering::Relaxed);
    if b == b'\n' {
        let (seconds, nanos) = crate::time::get_monotonic_time_ns();
        LINE_MS[tail].store(seconds.saturating_mul(1000).saturating_add(nanos / 1_000_000), Ordering::Relaxed);
    }
    TAIL.store(tail + 1, Ordering::Release);
}
/// Drain in thread context; a bounded byte batch avoids monopolizing the render thread.
pub fn drain() -> bool {
    if !crate::arch_interrupts_enabled() { return false; }
    let Some(mut scan) = SCAN.try_lock() else { return false; };
    let end = TAIL.load(Ordering::Acquire).min(scan.0 + 8192);
    scan_to(&mut scan, end)
}
fn scan_to(scan: &mut Scan, end: usize) -> bool {
    let mut changed = false;
    while scan.0 < end {
        let b = SERIAL[scan.0].load(Ordering::Relaxed);
        scan.0 += 1;
        let n = scan.2;
        if b == b'\n' {
            if n <= 1024 {
                let bytes = &scan.1[..n];
                changed |= scan.3.observe(bytes, LINE_MS[scan.0 - 1].load(Ordering::Relaxed));
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
    changed
}
/// Bring a snapshot up to the serial prefix observed on entry, even when it spans batches.
fn drain_snapshot() {
    let end = TAIL.load(Ordering::Acquire);
    if let Some(mut scan) = SCAN.try_lock() {
        scan_to(&mut scan, end);
    } else {
        // A syscall can preempt the render thread while it owns SCAN, then run
        // with preemption disabled. Parse the immutable prefix independently
        // instead of waiting for that thread to run. Reached bits are atomic.
        let mut scan = (0, [0; 1024], 0, crate::console::status::SuiteStatus::new());
        scan_to(&mut scan, end);
    }
}
/// Complete serial prefix, not the matcher's interpretation of it. Never silently truncate.
pub fn serial() -> Result<alloc::string::String, i32> {
    if OVERFLOW.load(Ordering::Acquire) { return Err(-75); }
    let n = TAIL.load(Ordering::Acquire);
    let bytes: alloc::vec::Vec<u8> = SERIAL[..n].iter().map(|b| b.load(Ordering::Relaxed)).collect();
    Ok(alloc::string::String::from_utf8_lossy(&bytes).into_owned())
}
/// Independent producer evidence: complete captured lines as `milliseconds|line`.
/// The timestamps come from the writer before publication. Keep the original, unannotated serial endpoint unchanged.
pub fn serial_timed() -> Result<alloc::string::String, i32> {
    use core::fmt::Write;
    if OVERFLOW.load(Ordering::Acquire) { return Err(-75); }
    let n = TAIL.load(Ordering::Acquire);
    let serial: alloc::vec::Vec<u8> = SERIAL[..n].iter().map(|b| b.load(Ordering::Relaxed)).collect();
    let mut out = alloc::string::String::new();
    let mut offset = 0;
    for line in serial.split_inclusive(|&b| b == b'\n') {
        offset += line.len();
        if !line.ends_with(b"\n") { break; }
        let ms = LINE_MS[offset - 1].load(Ordering::Relaxed);
        let _ = write!(out, "{ms}|{}", alloc::string::String::from_utf8_lossy(line));
    }
    Ok(out)
}
pub fn reached() -> alloc::vec::Vec<bool> {
    drain_snapshot();
    REACHED[..MARKERS.len()]
        .iter()
        .map(|v| v.load(Ordering::Acquire))
        .collect()
}
pub fn records() -> alloc::string::String {
    drain_snapshot();
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

/// Suite state observed by the same complete-line scanner as the stage producers.
/// Rendering calls this in thread context, outside UART ownership.
pub fn suite_status() -> Option<crate::console::status::SuiteStatus> {
    SCAN.try_lock().map(|scan| scan.3.clone())
}
