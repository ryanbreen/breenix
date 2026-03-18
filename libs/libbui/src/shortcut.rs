//! Keyboard shortcut definitions and display formatting.

/// A keyboard shortcut (modifier + key).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shortcut {
    pub key: u8,   // ASCII key (uppercase). 0 = no shortcut.
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
}

impl Shortcut {
    pub const NONE: Self = Self { key: 0, ctrl: false, shift: false, alt: false };

    pub const fn ctrl(key: u8) -> Self {
        Self { key, ctrl: true, shift: false, alt: false }
    }

    pub const fn key(key: u8) -> Self {
        Self { key, ctrl: false, shift: false, alt: false }
    }

    pub const fn ctrl_shift(key: u8) -> Self {
        Self { key, ctrl: true, shift: true, alt: false }
    }

    pub const fn alt(key: u8) -> Self {
        Self { key, ctrl: false, shift: false, alt: true }
    }

    pub fn is_some(&self) -> bool {
        self.key != 0
    }

    /// Format into a stack buffer: "Ctrl+S", "Shift+Z", etc. Returns byte count.
    pub fn format(&self, buf: &mut [u8; 24]) -> usize {
        if self.key == 0 {
            return 0;
        }
        let mut pos = 0;
        if self.ctrl {
            buf[pos..pos + 5].copy_from_slice(b"Ctrl+");
            pos += 5;
        }
        if self.shift {
            buf[pos..pos + 6].copy_from_slice(b"Shift+");
            pos += 6;
        }
        if self.alt {
            buf[pos..pos + 4].copy_from_slice(b"Alt+");
            pos += 4;
        }
        let display_key = if self.key >= b'a' && self.key <= b'z' {
            self.key - 32
        } else {
            self.key
        };
        buf[pos] = display_key;
        pos + 1
    }
}
