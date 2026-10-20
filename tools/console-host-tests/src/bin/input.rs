//! Run with `cargo run --manifest-path tools/console-host-tests/Cargo.toml --bin input`.
//! The real input core and key consumer, with an observable terminal sink.
// Included production modules expose APIs this focused host harness does not call.
#![allow(dead_code)]
use std::sync::{atomic::{AtomicBool, AtomicUsize, Ordering}, Mutex};

#[path = "../../../../kernel/src/input/mod.rs"]
mod input;
#[path = "../../../../kernel/src/console/keymap.rs"]
mod keymap;
#[path = "../../../../kernel/src/console/keys.rs"]
mod keys;
#[path = "../../../../kernel/src/tty/keyboard_input.rs"]
mod keyboard_input;
#[path = "../../../../kernel/src/tty/input_source.rs"]
mod input_source;
pub use input_source::InputSource;
use console::{splash, vt};

#[cfg(target_arch = "aarch64")]
#[path = "../../../../kernel/src/drivers/hid_boot/pointer.rs"]
pub mod hid_pointer;
#[cfg(target_arch = "aarch64")]
mod drivers {
    pub mod hid_boot {
        pub use crate::hid_pointer as pointer;
    }
}
#[cfg(target_arch = "aarch64")]
mod graphics {
    pub mod arm64_fb {
        pub struct FramebufferInfo {
            pub width: usize,
            pub height: usize,
        }
        // The keyboard harness has no firmware framebuffer.
        pub static FB_INFO_CACHE: std::sync::OnceLock<FramebufferInfo> = std::sync::OnceLock::new();
    }
}
#[cfg(target_arch = "aarch64")]
mod syscall {
    pub mod graphics {
        // There is no compositor thread in this keyboard harness.
        pub fn wake_compositor_if_waiting() {}
        pub fn wake_compositor_for_input() {}
    }
}

mod tty {
    pub use crate::InputSource;
    pub fn push_char_nonblock_from(byte: u8, _: InputSource) -> bool {
        if !crate::ACCEPT.load(crate::Ordering::Relaxed) { return false; }
        crate::BYTES.lock().unwrap().push(byte);
        true
    }
}
mod console {
    pub(crate) use crate::keys;
    pub mod splash { pub fn key() -> bool { false } }
    pub mod vt {
        pub const VT_COUNT: usize = 6;
        pub fn foreground() -> usize { crate::FG.load(crate::Ordering::Relaxed) }
        pub fn foreground_graphics() -> bool { crate::GRAPHICS.load(crate::Ordering::Relaxed) }
        pub fn key_switch_pending() -> Option<usize> { None }
        pub fn key_switch(n: usize, _: u8) { crate::FG.store(n, crate::Ordering::Relaxed); }
        pub fn size(_: usize) -> (usize, usize) { (80, 25) }
        pub fn scroll_view(_: usize, _: isize) {}
    }
}
static BYTES: Mutex<Vec<u8>> = Mutex::new(Vec::new());
static ACCEPT: AtomicBool = AtomicBool::new(true);
static FG: AtomicUsize = AtomicUsize::new(1);
static GRAPHICS: AtomicBool = AtomicBool::new(false);
fn received(want: &[u8]) { assert_eq!(BYTES.lock().unwrap().drain(..).collect::<Vec<_>>(), want); }
fn main() {
    use input::{Device::UsbBootKeyboard as Usb, report_key as key};
    use input::event::*;
    let (a, b) = (Usb(1), Usb(2));
    // Super is its own key: Ctrl neither sets the graphics ABI's Super bit nor taps it.
    key(a, KEY_LEFTCTRL, PRESS);
    assert_eq!(input::poll_modifier_state() & 10, 2);
    key(a, KEY_LEFTCTRL, RELEASE);
    assert!(!input::has_pending_super_tap());
    for _ in 0..2 {
        key(a, KEY_LEFTMETA, PRESS);
        assert_eq!(input::poll_modifier_state() & 10, 8);
        key(a, KEY_LEFTMETA, REPEAT);
        key(a, KEY_LEFTMETA, RELEASE);
        assert_eq!(input::poll_modifier_state() & 10, 0);
    }
    assert!(input::has_pending_super_tap());
    assert_eq!(input::take_super_tap_count(), 2);
    assert!(!input::has_pending_super_tap());

    // A graphics terminal gets Super in the key's bytes; Ctrl+Return and Ctrl+M are Enter.
    GRAPHICS.store(true, Ordering::Relaxed);
    key(a, KEY_LEFTMETA, PRESS);
    key(a, KEY_ENTER, PRESS);
    key(a, KEY_ENTER, REPEAT);
    key(a, KEY_ENTER, RELEASE);
    key(a, KEY_LEFTMETA, RELEASE);
    received(b"\x1b[13;9u\x1b[13;9:2u");
    assert_eq!(input::take_super_tap_count(), 1);
    key(a, KEY_LEFTCTRL, PRESS);
    key(a, KEY_ENTER, PRESS);
    key(a, KEY_ENTER, RELEASE);
    key(a, KEY_Z + 6, PRESS);
    key(a, KEY_Z + 6, RELEASE);
    key(a, KEY_LEFTCTRL, RELEASE);
    received(b"\r\r");
    assert!(!input::has_pending_super_tap());
    GRAPHICS.store(false, Ordering::Relaxed);

    // A release (including detach's release_all) cannot clear another device's Shift.
    key(a, KEY_LEFTSHIFT, PRESS);
    key(b, KEY_LEFTSHIFT, PRESS);
    key(a, KEY_LEFTSHIFT, RELEASE);
    assert_eq!(key(b, KEY_A, PRESS), 1);
    received(b"A");
    key(b, KEY_LEFTSHIFT, RELEASE);
    key(b, KEY_A, RELEASE);

    // Reserve A's F2. B's ordinary F2 and release must neither inherit nor clear it.
    key(a, KEY_LEFTCTRL, PRESS);
    key(a, KEY_LEFTALT, PRESS);
    key(a, KEY_F1 + 1, PRESS);
    key(a, KEY_LEFTCTRL, RELEASE);
    key(a, KEY_LEFTALT, RELEASE);
    key(b, KEY_F1 + 1, PRESS);
    received(b"\x1bOQ");
    key(b, KEY_F1 + 1, RELEASE);
    FG.store(1, Ordering::Relaxed);
    key(a, KEY_F1 + 1, REPEAT);
    key(a, KEY_F1 + 1, RELEASE);
    received(b"");
    assert_eq!(FG.load(Ordering::Relaxed), 1);

    // Consumed Alt bindings stay consumed after Alt is released and KD mode changes.
    key(a, KEY_LEFTALT, PRESS);
    key(a, KEY_F1 + 1, PRESS);
    key(a, KEY_LEFTALT, RELEASE);
    key(a, KEY_F1 + 1, REPEAT);
    GRAPHICS.store(true, Ordering::Relaxed);
    key(a, KEY_F1 + 1, REPEAT);
    key(a, KEY_F1 + 1, RELEASE);
    received(b"");
    key(a, KEY_F1 + 1, PRESS);
    received(b"\x1bOQ");
    key(a, KEY_F1 + 1, RELEASE);
    GRAPHICS.store(false, Ordering::Relaxed);

    // The consumer's count records sink acceptance, not generated bytes.
    ACCEPT.store(false, Ordering::Relaxed);
    assert_eq!(key(a, KEY_A, PRESS), 0);
    received(b"");

    // A refused discipline retains the byte and its source for exactly one later admission.
    let mut pending = keyboard_input::PendingInput::new();
    assert!(pending.push(b'x', InputSource::UsbKeyboard));
    pending.drain(|_, _| false);
    let mut got = Vec::new();
    pending.drain(|byte, source| { got.push((byte, source as u8)); true });
    pending.drain(|_, _| panic!("duplicate input"));
    assert_eq!(got, [(b'x', InputSource::UsbKeyboard as u8)]);
    for _ in 0..256 { assert!(pending.push(b'x', InputSource::UsbKeyboard)); }
    assert!(!pending.push(b'y', InputSource::UsbKeyboard));
    println!("input: device modifiers, Super, reservations, consumed repeats, admission count and retry passed");
}
