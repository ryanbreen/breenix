//! PS/2 scan code set 1 to key events.
//!
//! The i8042 keyboard sends set-1 bytes: a make code per press, the same code
//! with bit 7 set per release, `E0` before the extended keys, `E1` before the
//! six-byte Pause sequence, and the make code again while a key is held
//! (typematic). The key codes are the published `input-event-codes.h` values;
//! for the base keys they are the make codes themselves. Pure and
//! allocation-free: `rustc --edition=2021 --test set1.rs` runs its tests.

/// An event's value, as the input core takes it.
const RELEASE: i32 = 0;
const PRESS: i32 = 1;
const REPEAT: i32 = 2;

pub struct Decoder {
    /// `E0` came before this byte.
    extended: bool,
    /// Bytes of the Pause sequence still to come.
    skip: u8,
    /// The keys down, one bit per key code, so a held key's makes are repeats.
    down: [u64; 2],
}

impl Decoder {
    pub const fn new() -> Self {
        Self { extended: false, skip: 0, down: [0; 2] }
    }

    /// One byte from the keyboard: the key code and value it completes, if any.
    pub fn byte(&mut self, byte: u8) -> Option<(u16, i32)> {
        if self.skip != 0 {
            self.skip -= 1;
            return None;
        }
        match byte {
            0xE0 => {
                self.extended = true;
                return None;
            }
            0xE1 => {
                // Pause sends E1 1D 45 E1 9D C5 and has no key code here.
                self.extended = false;
                self.skip = 5;
                return None;
            }
            _ => {}
        }
        let extended = core::mem::replace(&mut self.extended, false);
        let make = byte & 0x7f;
        let code = if extended { extended_key(make) } else { base_key(make) }?;
        let (word, bit) = ((code / 64) as usize, 1u64 << (code % 64));
        let value = if byte & 0x80 != 0 {
            self.down[word] &= !bit;
            RELEASE
        } else if self.down[word] & bit != 0 {
            REPEAT
        } else {
            self.down[word] |= bit;
            PRESS
        };
        Some((code, value))
    }
}

/// Make codes 01-53 and 56-58 are the key codes of the same number (Escape
/// to keypad '.', the non-US backslash key, F11 and F12).
fn base_key(make: u8) -> Option<u16> {
    match make {
        0x01..=0x53 | 0x56..=0x58 => Some(make as u16),
        _ => None,
    }
}

/// The extended keys: keypad Enter and '/', the right Ctrl and Alt, the
/// navigation block and the Super keys. The others (the fake shifts some
/// keys send around themselves, Print Screen) have no key code here.
fn extended_key(make: u8) -> Option<u16> {
    Some(match make {
        0x1C => 96,  // KEY_KPENTER
        0x1D => 97,  // KEY_RIGHTCTRL
        0x35 => 98,  // KEY_KPSLASH
        0x38 => 100, // KEY_RIGHTALT
        0x47 => 102, // KEY_HOME
        0x48 => 103, // KEY_UP
        0x49 => 104, // KEY_PAGEUP
        0x4B => 105, // KEY_LEFT
        0x4D => 106, // KEY_RIGHT
        0x4F => 107, // KEY_END
        0x50 => 108, // KEY_DOWN
        0x51 => 109, // KEY_PAGEDOWN
        0x52 => 110, // KEY_INSERT
        0x53 => 111, // KEY_DELETE
        0x5B => 125, // KEY_LEFTMETA
        0x5C => 126, // KEY_RIGHTMETA
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(decoder: &mut Decoder, bytes: &[u8]) -> Vec<(u16, i32)> {
        bytes.iter().filter_map(|&b| decoder.byte(b)).collect()
    }

    #[test]
    fn makes_and_breaks_are_presses_and_releases_of_the_same_code() {
        let mut d = Decoder::new();
        // A, then Left Shift held around B, then F12 and the non-US backslash key.
        assert_eq!(
            feed(&mut d, &[0x1E, 0x9E, 0x2A, 0x30, 0xB0, 0xAA, 0x58, 0xD8, 0x56]),
            vec![(30, PRESS), (30, RELEASE), (42, PRESS), (48, PRESS), (48, RELEASE), (42, RELEASE), (88, PRESS), (88, RELEASE), (86, PRESS)]
        );
    }

    #[test]
    fn a_held_key_repeats_until_its_release() {
        let mut d = Decoder::new();
        assert_eq!(feed(&mut d, &[0x3C, 0x3C, 0x3C, 0xBC, 0x3C]), vec![(60, PRESS), (60, REPEAT), (60, REPEAT), (60, RELEASE), (60, PRESS)]);
    }

    #[test]
    fn extended_keys_take_their_own_codes() {
        let mut d = Decoder::new();
        // Right Alt + Left arrow, then Page Up, Delete and the right Ctrl.
        assert_eq!(
            feed(&mut d, &[0xE0, 0x38, 0xE0, 0x4B, 0xE0, 0xCB, 0xE0, 0xB8, 0xE0, 0x49, 0xE0, 0x53, 0xE0, 0x1D]),
            vec![(100, PRESS), (105, PRESS), (105, RELEASE), (100, RELEASE), (104, PRESS), (111, PRESS), (97, PRESS)]
        );
        // E0 applies to one byte only: the plain Left Alt after it is code 56.
        assert_eq!(feed(&mut d, &[0x38]), vec![(56, PRESS)]);
    }

    #[test]
    fn fake_shifts_print_screen_and_pause_send_nothing() {
        let mut d = Decoder::new();
        // Print Screen: E0 2A E0 37, released E0 B7 E0 AA; Pause: E1 1D 45 E1 9D C5.
        assert!(feed(&mut d, &[0xE0, 0x2A, 0xE0, 0x37, 0xE0, 0xB7, 0xE0, 0xAA]).is_empty());
        assert!(feed(&mut d, &[0xE1, 0x1D, 0x45, 0xE1, 0x9D, 0xC5]).is_empty());
        // The decoder is idle again: the next byte is a key.
        assert_eq!(feed(&mut d, &[0x1D]), vec![(29, PRESS)]);
    }
}
