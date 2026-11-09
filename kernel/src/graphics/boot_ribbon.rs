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
// Capture 132025 encoded about 158.5 KB before the first START and about
// 698 KB from BreenGL START to panel/kernel-screens (#2824).
const HEAD: usize = 192 * 1024;
const RING: usize = 1024 * 1024 - HEAD;
static BOOT_HEAD: [AtomicU8; HEAD] = [const { AtomicU8::new(0) }; HEAD];
static SERIAL: [AtomicU8; RING] = [const { AtomicU8::new(0) }; RING];
static TAIL: AtomicUsize = AtomicUsize::new(0);
static FIRST_START: AtomicUsize = AtomicUsize::new(usize::MAX);
static LATEST_START: AtomicUsize = AtomicUsize::new(usize::MAX);
static LAPPED: AtomicBool = AtomicBool::new(false);
fn put(offset: usize, byte: u8) {
    SERIAL[offset % RING].store(byte, Ordering::Relaxed);
    if offset < HEAD { BOOT_HEAD[offset].store(byte, Ordering::Relaxed); }
}
fn at(offset: usize) -> u8 {
    if offset < HEAD { BOOT_HEAD[offset].load(Ordering::Relaxed) }
    else { SERIAL[offset % RING].load(Ordering::Relaxed) }
}
type Scan = (usize, [u8; 1024], usize, crate::console::status::SuiteStatus);
static SCAN: spin::Mutex<Scan> = spin::Mutex::new((0, [0; 1024], 0, crate::console::status::SuiteStatus::new()));
/// Constant work per byte under UART ownership, including before the framebuffer exists.
pub fn byte(b: u8) {
    let tail = TAIL.load(Ordering::Relaxed);
    let size = if b == b'\n' { 9 } else { 1 };
    put(tail, b);
    if b == b'\n' {
        let (seconds, nanos) = crate::time::get_monotonic_time_ns();
        let ms = seconds.saturating_mul(1000).saturating_add(nanos / 1_000_000);
        for (i, byte) in ms.to_le_bytes().into_iter().enumerate() {
            put(tail + 1 + i, byte);
        }
    }
    TAIL.store(tail + size, Ordering::Release);
}
/// Drain in thread context; a bounded byte batch avoids monopolizing the render thread.
pub fn drain() -> bool {
    if !crate::arch_interrupts_enabled() { return false; }
    let Some(mut scan) = SCAN.try_lock() else { return false; };
    let end = TAIL.load(Ordering::Acquire).min(scan.0 + 8192);
    scan_to(&mut scan, end, true)
}
fn scan_to(scan: &mut Scan, end: usize, live: bool) -> bool {
    let mut changed = false;
    while scan.0 < end {
        if live && (LAPPED.load(Ordering::Acquire)
            || TAIL.load(Ordering::Acquire).saturating_sub(scan.0) > RING - 9) {
            LAPPED.store(true, Ordering::Release);
            break;
        }
        let b = at(scan.0);
        scan.0 += 1;
        let n = scan.2;
        if b == b'\n' {
            let ms = recorded_ms(&mut scan.0);
            if n <= 1024 {
                let bytes = &scan.1[..n];
                let start = scan.0 - n - 9;
                let observed = scan.3.observe(bytes, ms);
                changed |= observed;
                let is_start = observed && bytes.split(|b| *b == b' ').nth(2) == Some(b"START".as_slice());
                if is_start {
                    FIRST_START.fetch_min(start, Ordering::AcqRel);
                    // The independent snapshot can scan an older prefix after the live scanner.
                    let _ = LATEST_START.fetch_update(Ordering::AcqRel, Ordering::Acquire, |old| {
                        Some(if old == usize::MAX { start } else { old.max(start) })
                    });
                }
                if start < FIRST_START.load(Ordering::Acquire) {
                for (i, marker) in MARKERS.iter().enumerate() {
                    if super::boot_producer::matches(bytes, marker, MARKERS) {
                        changed |= !REACHED[i].swap(true, Ordering::AcqRel);
                    }
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
        scan_to(&mut scan, end, true);
    } else {
        // A syscall can preempt the render thread while it owns SCAN, then run
        // with preemption disabled. Parse the immutable prefix independently
        // instead of waiting for that thread to run. Reached bits are atomic.
        let mut scan = (0, [0; 1024], 0, crate::console::status::SuiteStatus::new());
        scan_to(&mut scan, end.min(HEAD - 8), false);
    }
}
/// Complete serial prefix, not the matcher's interpretation of it. Never silently truncate.
pub fn serial() -> Result<alloc::string::String, i32> {
    let n = TAIL.load(Ordering::Acquire);
    if n > RING { return Err(-75); }
    let mut bytes = alloc::vec::Vec::with_capacity(n);
    let mut offset = 0;
    while offset < n {
        let byte = at(offset);
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
    let n = TAIL.load(Ordering::Acquire);
    if n > RING { return Err(-75); }
    let mut line = alloc::vec::Vec::new();
    let mut out = alloc::string::String::new();
    let mut offset = 0;
    while offset < n {
        let byte = at(offset);
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
    let bytes = core::array::from_fn(|i| at(*offset + i));
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
    if LAPPED.load(Ordering::Acquire) { return None; }
    SCAN.try_lock().and_then(|scan| {
        if LAPPED.load(Ordering::Acquire) { None } else { Some(scan.3.clone()) }
    })
}
