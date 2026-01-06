use super::scancodes::{Key, *};

#[derive(Debug, Clone, Copy)]
pub struct Modifiers {
    pub left_shift: bool,
    pub right_shift: bool,
    pub left_ctrl: bool,
    pub right_ctrl: bool,
    pub left_alt: bool,
    pub right_alt: bool,
    pub left_cmd: bool,
    pub right_cmd: bool,
    pub caps_lock: bool,
}

impl Modifiers {
    pub const fn new() -> Self {
        Self {
            left_shift: false,
            right_shift: false,
            left_ctrl: false,
            right_ctrl: false,
            left_alt: false,
            right_alt: false,
            left_cmd: false,
            right_cmd: false,
            caps_lock: false,
        }
    }

    /// Returns true if any shift key is pressed
    pub fn shift(&self) -> bool {
        self.left_shift || self.right_shift
    }

    /// Returns true if any ctrl key is pressed
    pub fn ctrl(&self) -> bool {
        self.left_ctrl || self.right_ctrl
    }

    /// Returns true if any alt key is pressed
    pub fn alt(&self) -> bool {
        self.left_alt || self.right_alt
    }

    /// Update modifier state based on scancode
    /// Returns true if this was a modifier key
    pub fn update(&mut self, scancode: u8) -> bool {
        match scancode {
            LEFT_SHIFT_PRESSED => {
                self.left_shift = true;
                true
            }
            LEFT_SHIFT_RELEASED => {
                self.left_shift = false;
                true
            }
            RIGHT_SHIFT_PRESSED => {
                self.right_shift = true;
                true
            }
            RIGHT_SHIFT_RELEASED => {
                self.right_shift = false;
                true
            }
            LEFT_CTRL_PRESSED => {
                self.left_ctrl = true;
                true
            }
            LEFT_CTRL_RELEASED => {
                self.left_ctrl = false;
                true
            }
            LEFT_ALT_PRESSED => {
                self.left_alt = true;
                true
            }
            LEFT_ALT_RELEASED => {
                self.left_alt = false;
                true
            }
            CAPS_LOCK_PRESSED => {
                self.caps_lock = !self.caps_lock;
                true
            }
            LEFT_CMD_PRESSED => {
                self.left_cmd = true;
                true
            }
            LEFT_CMD_RELEASED => {
                self.left_cmd = false;
                true
            }
            RIGHT_CMD_PRESSED => {
                self.right_cmd = true;
                true
            }
            RIGHT_CMD_RELEASED => {
                self.right_cmd = false;
                true
            }
            _ => false,
        }
    }

    /// Apply modifiers to a key to get the actual character
    pub fn apply_to(&self, key: Key) -> char {
        // Check if this is an alphabetic key
        let is_alphabetic = (0x10 <= key.scancode && key.scancode <= 0x19)  // Q-P
            || (0x1E <= key.scancode && key.scancode <= 0x26)  // A-L
            || (0x2C <= key.scancode && key.scancode <= 0x32); // Z-M

        // Handle Ctrl modifier for alphabetic keys
        // Ctrl+A = 0x01, Ctrl+B = 0x02, ..., Ctrl+Z = 0x1A
        if self.ctrl() && is_alphabetic {
            // Get the base letter (lowercase)
            let base = key.lower;
            // Convert to control character: 'a' (0x61) -> 0x01, 'z' (0x7A) -> 0x1A
            let ctrl_char = (base as u8) - 0x60;
            return char::from(ctrl_char);
        }

        if is_alphabetic {
            // For alphabetic keys, caps lock XORs with shift
            if self.shift() ^ self.caps_lock {
                key.upper
            } else {
                key.lower
            }
        } else {
            // For non-alphabetic keys, only shift matters
            if self.shift() {
                key.upper
            } else {
                key.lower
            }
        }
    }
}
