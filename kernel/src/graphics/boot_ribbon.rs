//! Producer-observed boot stages, preserved independently of display ownership.
//! UART ownership serializes bytes; updates use atomics and never draw on the writer path.
use core::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};
const MARKERS: &[&str] = include!(concat!(env!("OUT_DIR"), "/boot_ribbon.rs"));
static REACHED: [AtomicBool; MARKERS.len()] = [const { AtomicBool::new(false) }; MARKERS.len()];
// UART ownership gives one producer. Preserve the serial bytes independently of REACHED;
// matching and rendering happen only in thread context with IRQs enabled.
// Timestamp bytes follow each newline in this private capture encoding. The
// raw serial endpoint strips them. A 1 MiB byte store bounds the writable image
// without reserving a timestamp word for byte positions that are not newlines.
const CAPACITY: usize = 1024 * 1024;
static SERIAL: [AtomicU8; CAPACITY] = [const { AtomicU8::new(0) }; CAPACITY];
static TAIL: AtomicUsize = AtomicUsize::new(0);
static OVERFLOW: AtomicBool = AtomicBool::new(false);
type Scan = (usize, [u8; 1024], usize, crate::console::status::SuiteStatus);
static SCAN: spin::Mutex<Scan> = spin::Mutex::new((0, [0; 1024], 0, crate::console::status::SuiteStatus::new()));
/// Constant work per byte under UART ownership, including before the framebuffer exists.
pub fn byte(b: u8) {
    if OVERFLOW.load(Ordering::Relaxed) { return; }
    let tail = TAIL.load(Ordering::Relaxed);
    let size = if b == b'\n' { 9 } else { 1 };
    if tail + size > CAPACITY { OVERFLOW.store(true, Ordering::Release); return; }
    SERIAL[tail].store(b, Ordering::Relaxed);
    if b == b'\n' {
        let (seconds, nanos) = crate::time::get_monotonic_time_ns();
        let ms = seconds.saturating_mul(1000).saturating_add(nanos / 1_000_000);
        for (i, byte) in ms.to_le_bytes().into_iter().enumerate() {
            SERIAL[tail + 1 + i].store(byte, Ordering::Relaxed);
        }
    }
    TAIL.store(tail + size, Ordering::Release);
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
            let ms = recorded_ms(&mut scan.0);
            if n <= 1024 {
                let bytes = &scan.1[..n];
                changed |= scan.3.observe(bytes, ms);
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
    let mut bytes = alloc::vec::Vec::with_capacity(n);
    let mut offset = 0;
    while offset < n {
        let byte = SERIAL[offset].load(Ordering::Relaxed);
        offset += 1;
        bytes.push(byte);
        if byte == b'\n' { offset += 8; }
    }
    Ok(alloc::string::String::from_utf8_lossy(&bytes).into_owned())
}
/// Independent producer evidence: complete captured lines as `milliseconds|line`.
/// The timestamps come from the writer before publication. Keep the original, unannotated serial endpoint unchanged.
pub fn serial_timed() -> Result<alloc::string::String, i32> {
    use core::fmt::Write;
    if OVERFLOW.load(Ordering::Acquire) { return Err(-75); }
    let n = TAIL.load(Ordering::Acquire);
    let mut line = alloc::vec::Vec::new();
    let mut out = alloc::string::String::new();
    let mut offset = 0;
    while offset < n {
        let byte = SERIAL[offset].load(Ordering::Relaxed);
        offset += 1;
        line.push(byte);
        if byte == b'\n' {
            let ms = recorded_ms(&mut offset);
            let _ = write!(out, "{ms}|{}", alloc::string::String::from_utf8_lossy(&line));
            line.clear();
        }
    }
    Ok(out)
}
// TAIL publishes a newline together with its eight timestamp bytes.
fn recorded_ms(offset: &mut usize) -> u64 {
    let bytes = core::array::from_fn(|i| SERIAL[*offset + i].load(Ordering::Relaxed));
    *offset += 8;
    u64::from_le_bytes(bytes)
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
