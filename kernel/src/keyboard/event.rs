use super::modifiers::Modifiers;

#[derive(Debug, Clone, Copy)]
pub struct KeyEvent {
    pub character: Option<char>,
    pub ctrl: bool,
}

impl KeyEvent {
    pub fn new(_scancode: u8, character: Option<char>, modifiers: &Modifiers) -> Self {
        Self {
            character,
            ctrl: modifiers.ctrl(),
        }
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

    /// Check if this is Ctrl+T (time debug)
    pub fn is_ctrl_t(&self) -> bool {
        self.ctrl && self.character == Some('t')
    }

    /// Check if this is Ctrl+M (memory debug)
    pub fn is_ctrl_m(&self) -> bool {
        self.ctrl && self.character == Some('m')
    }
    
    
    /// Generic Ctrl+key check
    pub fn is_ctrl_key(&self, key: char) -> bool {
        self.ctrl && self.character == Some(key)
    }
}