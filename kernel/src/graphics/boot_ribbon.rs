//! Producer-observed boot stages, preserved independently of display ownership.
//! UART ownership serializes bytes; updates use atomics and never draw on the writer path.
use core::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};
const MARKERS: &[&str] = include!(concat!(env!("OUT_DIR"), "/boot_ribbon.rs"));
const LAST: &[(&str, &str)] = include!(concat!(env!("OUT_DIR"), "/boot_ribbon_last.rs"));
static MODE: AtomicUsize = AtomicUsize::new(0);
static REACHED: [AtomicBool; MARKERS.len()] = [const { AtomicBool::new(false) }; MARKERS.len()];
// UART ownership gives one producer. Preserve the serial bytes independently of REACHED;
// matching and rendering happen only in thread context with IRQs enabled.
const CAPACITY: usize = 512 * 1024;
static SERIAL: [AtomicU8; CAPACITY] = [const { AtomicU8::new(0) }; CAPACITY];
static TAIL: AtomicUsize = AtomicUsize::new(0);
static DIRTY: AtomicBool = AtomicBool::new(false);
static RETIRED: AtomicBool = AtomicBool::new(false);
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
                retire_at_last(bytes);
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
    // Prompts need not end with a newline. Match the current partial line as well.
    if scan.2 <= 1024 { retire_at_last(&scan.1[..scan.2]); }
    if changed { DIRTY.store(true, Ordering::Release); }
    changed
}
/// Selected after boot-target resolution, before the mode's final stages can run.
pub fn select_mode(mode: &str) {
    if let Some(index) = LAST.iter().position(|&(name, _)| name == mode) {
        MODE.store(index, Ordering::Release);
    }
}
fn retire_at_last(bytes: &[u8]) {
    let marker = LAST[MODE.load(Ordering::Acquire)].1;
    if super::boot_producer::matches(bytes, marker, MARKERS) { retire(); }
}
pub fn take_dirty() -> bool { DIRTY.swap(false, Ordering::AcqRel) }
/// B51: neither stage updates nor display give-back can revive a retired ribbon.
pub fn retire() {
    if !RETIRED.swap(true, Ordering::AcqRel) { DIRTY.store(true, Ordering::Release); }
}
pub fn is_booting() -> bool {
    !RETIRED.load(Ordering::Acquire)
}
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proc_stages_follow_complete_producer_lines() {
        let marker = MARKERS[0].split('|').next().unwrap();
        for b in marker.bytes() { byte(b); }
        drain();
        assert!(!REACHED[0].load(Ordering::Acquire));
        byte(b'\n');
        drain();
        assert!(records().starts_with(&alloc::format!("1|{}\n", MARKERS[0])));
    }
}
