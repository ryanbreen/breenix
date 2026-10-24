//! The console's key consumer (`docs/console/contract.md` sections 1.6, 5.5,
//! 7 and 9): the input core hands it each key event, and it acts on the
//! foreground terminal through the keymap (`keymap.rs`).
//!
//! It runs where the event was reported, in interrupt context or the xHCI
//! worker: it waits only on locks whose holders mask interrupts, and only tries
//! the others; it does not allocate and does not log. A switch is left for the render thread
//! (`vt::key_switch`); the scrollback view moves under the console's
//! interrupt-masked lock, as `VT_SCROLLVIEW` moves it; bytes go to the
//! foreground terminal's input.

use super::keymap::{self, Action, Lock, Target};
use super::vt::{self, VT_COUNT};
use crate::input::event::{Modifiers, PRESS, RELEASE, REPEAT};
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// Caps Lock, kept per terminal (section 9.1).
static CAPS: [AtomicBool; VT_COUNT] = [const { AtomicBool::new(false) }; VT_COUNT];
static NUM: AtomicBool = AtomicBool::new(false);
static SCROLL: AtomicBool = AtomicBool::new(false);

// Key codes in the console bindings fit in two words. Track each physical keyboard.
static CONSUMED: [[AtomicU64; 2]; 258] = [const { [const { AtomicU64::new(0) }; 2] }; 258];

/// One key event. Returns how many bytes it sent the foreground terminal.
pub fn key(device: usize, code: u16, value: i32, modifiers: Modifiers, reserved: bool, source: crate::tty::InputSource) -> usize {
    // A key press while the boot splash is up ends it and is otherwise ignored.
    if value == PRESS && super::splash::key() {
        return 0;
    }
    let fg = vt::foreground();
    let state = keymap::State {
        text: !vt::foreground_graphics(),
        caps: CAPS[fg - 1].load(Ordering::Relaxed),
        num: NUM.load(Ordering::Relaxed),
    };
    if code < 128 && device < CONSUMED.len() {
        let word = &CONSUMED[device][code as usize / 64];
        let bit = 1u64 << (code % 64);
        if value == RELEASE {
            word.fetch_and(!bit, Ordering::Relaxed);
        } else if value == REPEAT && word.load(Ordering::Relaxed) & bit != 0 {
            return 0;
        } else if value == PRESS && keymap::consumes(code, modifiers, reserved, state.text) {
            word.fetch_or(bit, Ordering::Relaxed);
        }
    }
    match keymap::action(code, value, modifiers, reserved, state) {
        Action::None => 0,
        Action::Bytes(bytes) => {
            bytes.as_slice().iter().filter(|&&byte| crate::tty::push_char_nonblock_from(byte, source)).count()
        }
        Action::Switch(target, chord) => {
            // Previous and next count from a switch a key asked for and the render thread has not made yet.
            let from = vt::key_switch_pending().unwrap_or(fg);
            let to = match target {
                Target::Vt(n) => n,
                Target::Previous => (from + VT_COUNT - 2) % VT_COUNT + 1,
                Target::Next => from % VT_COUNT + 1,
            };
            vt::key_switch(to, chord);
            0
        }
        Action::Scroll(back) => {
            let half = (vt::size(fg).1 / 2) as isize;
            vt::scroll_view(fg, if back { half } else { -half });
            0
        }
        Action::Lock(lock) => {
            let flag = match lock {
                Lock::Caps => &CAPS[fg - 1],
                Lock::Num => &NUM,
                Lock::Scroll => &SCROLL,
            };
            flag.fetch_xor(true, Ordering::Relaxed);
            0
        }
    }
}

/// The keyboard LEDs for the lock state: bit 0 Num Lock, bit 1 Caps Lock (the
/// foreground terminal's), bit 2 Scroll Lock, as a HID boot keyboard's LED
/// report orders them.
pub fn leds() -> u8 {
    let caps = CAPS[vt::foreground() - 1].load(Ordering::Relaxed);
    NUM.load(Ordering::Relaxed) as u8 | (caps as u8) << 1 | (SCROLL.load(Ordering::Relaxed) as u8) << 2
}

/// Discard consumed-key state when the input core drains a physical device.
/// Lock toggles belong to the terminal and deliberately survive custody changes.
#[cfg(target_arch = "aarch64")]
pub(crate) fn reset_device(device: usize) {
    if let Some(words) = CONSUMED.get(device) {
        for word in words { word.store(0, Ordering::Relaxed); }
    }
}
