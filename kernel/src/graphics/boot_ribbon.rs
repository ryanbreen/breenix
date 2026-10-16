//! Producer-observed boot stages, preserved independently of display ownership.
//! UART ownership serializes bytes; updates use atomics and never draw on the writer path.
use core::sync::atomic::{AtomicBool, Ordering};
static MARKERS: &[&str] = include!(concat!(env!("OUT_DIR"), "/boot_ribbon.rs"));
static REACHED: [AtomicBool; 416] = [const { AtomicBool::new(false) }; 416];
static LINE: spin::Mutex<([u8; 1024], usize)> = spin::Mutex::new(([0; 1024], 0));
static LOST: AtomicBool = AtomicBool::new(false);
/// Called only under the UART capture ownership, including before the framebuffer exists.
pub fn byte(b: u8) {
    let Some(mut line) = LINE.try_lock() else {
        LOST.store(true, Ordering::Relaxed);
        return;
    };
    let n = line.1;
    if b == b'\n' {
        if n <= 1024 && !LOST.swap(false, Ordering::Relaxed) {
            let bytes = &line.0[..n];
            let announces = [b"test_exec".as_slice(), b"will emit", b"->", b"marker if"]
                .iter()
                .any(|p| bytes.windows(p.len()).any(|w| w == *p));
            if !announces {
                for (i, marker) in MARKERS.iter().enumerate() {
                    if marker
                        .split('|')
                        .any(|m| !m.is_empty() && bytes.windows(m.len()).any(|w| w == m.as_bytes()))
                    {
                        REACHED[i].store(true, Ordering::Release);
                    }
                }
            }
        }
        line.1 = 0;
    } else {
        if n < 1024 {
            line.0[n] = b;
        }
        line.1 = n.saturating_add(1);
    }
}
pub fn reached() -> alloc::vec::Vec<bool> {
    REACHED[..MARKERS.len()]
        .iter()
        .map(|v| v.load(Ordering::Acquire))
        .collect()
}
pub fn records() -> alloc::string::String {
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
pub fn draw(pixels: &mut [u8]) {
    let Ok(mut fb) = (unsafe {
        libgfx::framebuf::FrameBuf::from_raw(pixels.as_mut_ptr(), 1280, 800, 5120, 4, true)
    }) else {
        return;
    };
    let mut stages = reached();
    if crate::test_control::take(breengel_abi::tc_point::RIBBON_DROP_STAGE, 0).is_some() {
        if let Some(s) = stages.iter_mut().find(|s| **s) {
            *s = false;
        }
    }
    let _ = libgfx::stage::draw_boot_ribbon(&mut fb, &stages);
}
