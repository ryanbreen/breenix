use super::modifiers::Modifiers;

#[derive(Debug, Clone, Copy)]
pub struct KeyEvent {
    pub character: Option<char>,
    pub ctrl: bool,
}

/// Convert a letter to its control character equivalent
/// 'a' -> 0x01, 'b' -> 0x02, ..., 'z' -> 0x1A
fn letter_to_ctrl_char(letter: char) -> char {
    // Handle both lowercase and uppercase
    let lower = if letter.is_ascii_uppercase() {
        letter.to_ascii_lowercase()
    } else {
        letter
    };
    // 'a' (0x61) - 0x60 = 0x01
    ((lower as u8) - 0x60) as char
}

impl KeyEvent {
    pub fn new(_scancode: u8, character: Option<char>, modifiers: &Modifiers) -> Self {
        Self {
            character,
            ctrl: modifiers.ctrl(),
        }
    }

    /// Check if this is Ctrl+C (SIGINT - now handled by TTY layer)
    #[allow(dead_code)]
    pub fn is_ctrl_c(&self) -> bool {
        // Ctrl+C produces 0x03
        self.ctrl && self.character == Some('\x03')
    }

    /// Check if this is Ctrl+D (EOF - now handled by TTY layer)
    #[allow(dead_code)]
    pub fn is_ctrl_d(&self) -> bool {
        // Ctrl+D produces 0x04
        self.ctrl && self.character == Some('\x04')
    }

    /// Check if this is Ctrl+S (suspend output - handled by TTY)
    #[allow(dead_code)]
    pub fn is_ctrl_s(&self) -> bool {
        // Ctrl+S produces 0x13
        self.ctrl && self.character == Some('\x13')
    }

    /// Check if this is Ctrl+T (time debug)
    pub fn is_ctrl_t(&self) -> bool {
        // Ctrl+T produces 0x14
        self.ctrl && self.character == Some('\x14')
    }

    /// Check if this is Ctrl+M (memory debug - now routed through TTY as regular input)
    #[allow(dead_code)]
    pub fn is_ctrl_m(&self) -> bool {
        // Ctrl+M produces 0x0D (same as carriage return)
        self.ctrl && self.character == Some('\x0D')
    }

    /// Check if this is Ctrl+U (userspace test)
    #[allow(dead_code)]
    pub fn is_ctrl_u(&self) -> bool {
        // Ctrl+U produces 0x15
        self.ctrl && self.character == Some('\x15')
    }

    /// Generic Ctrl+key check for letters a-z
    /// Accepts the letter (e.g., 'c') and checks if the character is the control equivalent (0x03)
    pub fn is_ctrl_key(&self, key: char) -> bool {
        if !self.ctrl {
            return false;
        }
        // Convert the key to its control character equivalent
        let ctrl_char = letter_to_ctrl_char(key);
        self.character == Some(ctrl_char)
    }
}
