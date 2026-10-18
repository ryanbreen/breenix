//! Key events: key codes, derived modifiers and the reserved console chords.
//!
//! Key codes and values are the published `input-event-codes.h` interface facts
//! (`docs/console/contract.md` section 9.1, GUI plan section 5.1). Pure and
//! allocation-free: `rustc --edition=2021 --test event.rs` runs its tests.

/// An event's value: a key went up, went down, or the device repeated it.
pub const RELEASE: i32 = 0;
pub const PRESS: i32 = 1;
pub const REPEAT: i32 = 2;

pub const KEY_ESC: u16 = 1;
pub const KEY_1: u16 = 2;
pub const KEY_0: u16 = 11;
pub const KEY_MINUS: u16 = 12;
pub const KEY_EQUAL: u16 = 13;
pub const KEY_BACKSPACE: u16 = 14;
pub const KEY_TAB: u16 = 15;
pub const KEY_Q: u16 = 16;
pub const KEY_LEFTBRACE: u16 = 26;
pub const KEY_RIGHTBRACE: u16 = 27;
pub const KEY_ENTER: u16 = 28;
pub const KEY_LEFTCTRL: u16 = 29;
pub const KEY_A: u16 = 30;
pub const KEY_SEMICOLON: u16 = 39;
pub const KEY_APOSTROPHE: u16 = 40;
pub const KEY_GRAVE: u16 = 41;
pub const KEY_LEFTSHIFT: u16 = 42;
pub const KEY_BACKSLASH: u16 = 43;
pub const KEY_Z: u16 = 44;
pub const KEY_COMMA: u16 = 51;
pub const KEY_DOT: u16 = 52;
pub const KEY_SLASH: u16 = 53;
pub const KEY_RIGHTSHIFT: u16 = 54;
pub const KEY_KPASTERISK: u16 = 55;
pub const KEY_LEFTALT: u16 = 56;
pub const KEY_SPACE: u16 = 57;
pub const KEY_CAPSLOCK: u16 = 58;
pub const KEY_F1: u16 = 59;
pub const KEY_F10: u16 = 68;
pub const KEY_NUMLOCK: u16 = 69;
pub const KEY_SCROLLLOCK: u16 = 70;
pub const KEY_KP7: u16 = 71;
pub const KEY_KP8: u16 = 72;
pub const KEY_KP9: u16 = 73;
pub const KEY_KPMINUS: u16 = 74;
pub const KEY_KP4: u16 = 75;
pub const KEY_KP5: u16 = 76;
pub const KEY_KP6: u16 = 77;
pub const KEY_KPPLUS: u16 = 78;
pub const KEY_KP1: u16 = 79;
pub const KEY_KP2: u16 = 80;
pub const KEY_KP3: u16 = 81;
pub const KEY_KP0: u16 = 82;
pub const KEY_KPDOT: u16 = 83;
pub const KEY_102ND: u16 = 86;
pub const KEY_F11: u16 = 87;
pub const KEY_F12: u16 = 88;
pub const KEY_KPENTER: u16 = 96;
pub const KEY_RIGHTCTRL: u16 = 97;
pub const KEY_KPSLASH: u16 = 98;
pub const KEY_RIGHTALT: u16 = 100;
pub const KEY_HOME: u16 = 102;
pub const KEY_UP: u16 = 103;
pub const KEY_PAGEUP: u16 = 104;
pub const KEY_LEFT: u16 = 105;
pub const KEY_RIGHT: u16 = 106;
pub const KEY_END: u16 = 107;
pub const KEY_DOWN: u16 = 108;
pub const KEY_PAGEDOWN: u16 = 109;
pub const KEY_INSERT: u16 = 110;
pub const KEY_DELETE: u16 = 111;
pub const KEY_KPEQUAL: u16 = 117;
pub const KEY_LEFTMETA: u16 = 125;
pub const KEY_RIGHTMETA: u16 = 126;

/// The function key's number, 1 to 12, for F1 to F12.
pub fn function_key(code: u16) -> Option<u8> {
    match code {
        KEY_F1..=KEY_F10 => Some((code - KEY_F1 + 1) as u8),
        KEY_F11 => Some(11),
        KEY_F12 => Some(12),
        _ => None,
    }
}

/// The modifier keys held, one bit each, derived from key events.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modifiers(pub u8);

const LEFT_CTRL: u8 = 1 << 0;
const RIGHT_CTRL: u8 = 1 << 1;
const LEFT_SHIFT: u8 = 1 << 2;
const RIGHT_SHIFT: u8 = 1 << 3;
const LEFT_ALT: u8 = 1 << 4;
const RIGHT_ALT: u8 = 1 << 5;
const LEFT_META: u8 = 1 << 6;
const RIGHT_META: u8 = 1 << 7;

impl Modifiers {
    pub fn ctrl(self) -> bool {
        self.0 & (LEFT_CTRL | RIGHT_CTRL) != 0
    }
    pub fn shift(self) -> bool {
        self.0 & (LEFT_SHIFT | RIGHT_SHIFT) != 0
    }
    pub fn alt(self) -> bool {
        self.0 & (LEFT_ALT | RIGHT_ALT) != 0
    }
    /// Super: a key of its own, distinct from Ctrl.
    pub fn meta(self) -> bool {
        self.0 & (LEFT_META | RIGHT_META) != 0
    }
}

/// The modifier bit a key code holds, if it is a modifier key.
pub fn modifier_bit(code: u16) -> Option<u8> {
    match code {
        KEY_LEFTCTRL => Some(LEFT_CTRL),
        KEY_RIGHTCTRL => Some(RIGHT_CTRL),
        KEY_LEFTSHIFT => Some(LEFT_SHIFT),
        KEY_RIGHTSHIFT => Some(RIGHT_SHIFT),
        KEY_LEFTALT => Some(LEFT_ALT),
        KEY_RIGHTALT => Some(RIGHT_ALT),
        KEY_LEFTMETA => Some(LEFT_META),
        KEY_RIGHTMETA => Some(RIGHT_META),
        _ => None,
    }
}

/// One device's held modifiers after an event of `code` with `value`.
pub fn update_modifiers(held: u8, code: u16, value: i32) -> u8 {
    match modifier_bit(code) {
        Some(bit) if value == RELEASE => held & !bit,
        Some(bit) => held | bit,
        None => held,
    }
}

/// The reserved console chords (contract section 9): Ctrl+Alt+F1 to F12 reach
/// the console under any compositor grab, and so do their releases, whatever
/// modifiers are still held then. `held` keeps one bit per function key whose
/// press was reserved.
pub fn reserved(held: &mut u16, code: u16, value: i32, modifiers: Modifiers) -> bool {
    let Some(n) = function_key(code) else {
        return false;
    };
    let bit = 1u16 << n;
    match value {
        PRESS if modifiers.ctrl() && modifiers.alt() => {
            *held |= bit;
            true
        }
        RELEASE => {
            let was = *held & bit != 0;
            *held &= !bit;
            was
        }
        _ => *held & bit != 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modifiers_are_derived_from_presses_and_releases() {
        let mut held = 0;
        held = update_modifiers(held, KEY_RIGHTALT, PRESS);
        held = update_modifiers(held, KEY_LEFTCTRL, PRESS);
        let m = Modifiers(held);
        assert!(m.alt() && m.ctrl() && !m.shift() && !m.meta());
        // A repeat of a held modifier keeps it held; a release of the other side does not drop it.
        held = update_modifiers(held, KEY_RIGHTALT, REPEAT);
        held = update_modifiers(held, KEY_LEFTALT, RELEASE);
        assert!(Modifiers(held).alt());
        held = update_modifiers(held, KEY_RIGHTALT, RELEASE);
        held = update_modifiers(held, KEY_A, PRESS);
        assert_eq!(Modifiers(held), Modifiers(LEFT_CTRL));
        // Super is its own key, not Ctrl.
        let m = Modifiers(update_modifiers(0, KEY_LEFTMETA, PRESS));
        assert!(m.meta() && !m.ctrl());
    }

    #[test]
    fn function_keys_number_one_to_twelve() {
        let numbers: Vec<u8> = (0..200).filter_map(function_key).collect();
        assert_eq!(numbers, (1..=12).collect::<Vec<u8>>());
        assert_eq!(function_key(KEY_F11), Some(11));
        assert_eq!(function_key(KEY_F12), Some(12));
    }

    #[test]
    fn ctrl_alt_function_keys_and_their_releases_are_reserved() {
        let ctrl_alt = Modifiers(LEFT_CTRL | RIGHT_ALT);
        let mut held = 0;
        assert!(reserved(&mut held, KEY_F1 + 2, PRESS, ctrl_alt));
        // The release is reserved even when Ctrl and Alt went up first.
        assert!(reserved(&mut held, KEY_F1 + 2, REPEAT, Modifiers(0)));
        assert!(reserved(&mut held, KEY_F1 + 2, RELEASE, Modifiers(0)));
        assert_eq!(held, 0);
        // F7 to F12 are reserved too.
        assert!(reserved(&mut held, KEY_F12, PRESS, ctrl_alt));
        assert!(reserved(&mut held, KEY_F12, RELEASE, ctrl_alt));
        // Alt+Fn, Ctrl+Fn, and other keys under Ctrl+Alt are not.
        assert!(!reserved(&mut held, KEY_F1, PRESS, Modifiers(LEFT_ALT)));
        assert!(!reserved(&mut held, KEY_F1, RELEASE, Modifiers(LEFT_ALT)));
        assert!(!reserved(&mut held, KEY_F1, PRESS, Modifiers(RIGHT_CTRL)));
        assert!(!reserved(&mut held, KEY_LEFT, PRESS, ctrl_alt));
        assert!(!reserved(&mut held, KEY_DELETE, PRESS, ctrl_alt));
        // A function key pressed before Ctrl+Alt went down stays ordinary through its release.
        assert!(!reserved(&mut held, KEY_F1 + 1, RELEASE, ctrl_alt));
    }
}
