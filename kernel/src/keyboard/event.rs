use super::modifiers::Modifiers;

#[derive(Debug, Clone, Copy)]
pub struct KeyEvent {
    pub scancode: u8,
    pub character: Option<char>,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub cmd: bool,
    pub caps_lock: bool,
}

impl KeyEvent {
    pub fn new(scancode: u8, character: Option<char>, modifiers: &Modifiers) -> Self {
        Self {
            scancode,
            character,
            ctrl: modifiers.ctrl(),
            alt: modifiers.alt(),
            shift: modifiers.shift(),
            cmd: modifiers.cmd(),
            caps_lock: modifiers.caps_lock,
        }
    }

    /// Check if this is a printable character event
    pub fn is_printable(&self) -> bool {
        self.character.is_some() && !self.ctrl && !self.alt && !self.cmd
    }

    /// Check if this is Ctrl+C
    pub fn is_ctrl_c(&self) -> bool {
        self.ctrl && self.character == Some('c')
    }

    /// Check if this is Ctrl+D
    pub fn is_ctrl_d(&self) -> bool {
        self.ctrl && self.character == Some('d')
    }

    /// Check if this is Ctrl+S
    pub fn is_ctrl_s(&self) -> bool {
        self.ctrl && self.character == Some('s')
    }
}