#[derive(Debug, Clone, Copy)]
pub struct Key {
    pub lower: char,
    pub upper: char,
    pub scancode: u8,
}

// Number keys
pub const ZERO_KEY: Key = Key {
    lower: '0',
    upper: ')',
    scancode: 0x0B,
};
pub const ONE_KEY: Key = Key {
    lower: '1',
    upper: '!',
    scancode: 0x02,
};
pub const TWO_KEY: Key = Key {
    lower: '2',
    upper: '@',
    scancode: 0x03,
};
pub const THREE_KEY: Key = Key {
    lower: '3',
    upper: '#',
    scancode: 0x04,
};
pub const FOUR_KEY: Key = Key {
    lower: '4',
    upper: '$',
    scancode: 0x05,
};
pub const FIVE_KEY: Key = Key {
    lower: '5',
    upper: '%',
    scancode: 0x06,
};
pub const SIX_KEY: Key = Key {
    lower: '6',
    upper: '^',
    scancode: 0x07,
};
pub const SEVEN_KEY: Key = Key {
    lower: '7',
    upper: '&',
    scancode: 0x08,
};
pub const EIGHT_KEY: Key = Key {
    lower: '8',
    upper: '*',
    scancode: 0x09,
};
pub const NINE_KEY: Key = Key {
    lower: '9',
    upper: '(',
    scancode: 0x0A,
};

// Letter keys - Row 1 (QWERTY)
pub const Q_KEY: Key = Key {
    lower: 'q',
    upper: 'Q',
    scancode: 0x10,
};
pub const W_KEY: Key = Key {
    lower: 'w',
    upper: 'W',
    scancode: 0x11,
};
pub const E_KEY: Key = Key {
    lower: 'e',
    upper: 'E',
    scancode: 0x12,
};
pub const R_KEY: Key = Key {
    lower: 'r',
    upper: 'R',
    scancode: 0x13,
};
pub const T_KEY: Key = Key {
    lower: 't',
    upper: 'T',
    scancode: 0x14,
};
pub const Y_KEY: Key = Key {
    lower: 'y',
    upper: 'Y',
    scancode: 0x15,
};
pub const U_KEY: Key = Key {
    lower: 'u',
    upper: 'U',
    scancode: 0x16,
};
pub const I_KEY: Key = Key {
    lower: 'i',
    upper: 'I',
    scancode: 0x17,
};
pub const O_KEY: Key = Key {
    lower: 'o',
    upper: 'O',
    scancode: 0x18,
};
pub const P_KEY: Key = Key {
    lower: 'p',
    upper: 'P',
    scancode: 0x19,
};

// Letter keys - Row 2 (ASDF)
pub const A_KEY: Key = Key {
    lower: 'a',
    upper: 'A',
    scancode: 0x1E,
};
pub const S_KEY: Key = Key {
    lower: 's',
    upper: 'S',
    scancode: 0x1F,
};
pub const D_KEY: Key = Key {
    lower: 'd',
    upper: 'D',
    scancode: 0x20,
};
pub const F_KEY: Key = Key {
    lower: 'f',
    upper: 'F',
    scancode: 0x21,
};
pub const G_KEY: Key = Key {
    lower: 'g',
    upper: 'G',
    scancode: 0x22,
};
pub const H_KEY: Key = Key {
    lower: 'h',
    upper: 'H',
    scancode: 0x23,
};
pub const J_KEY: Key = Key {
    lower: 'j',
    upper: 'J',
    scancode: 0x24,
};
pub const K_KEY: Key = Key {
    lower: 'k',
    upper: 'K',
    scancode: 0x25,
};
pub const L_KEY: Key = Key {
    lower: 'l',
    upper: 'L',
    scancode: 0x26,
};

// Letter keys - Row 3 (ZXCV)
pub const Z_KEY: Key = Key {
    lower: 'z',
    upper: 'Z',
    scancode: 0x2C,
};
pub const X_KEY: Key = Key {
    lower: 'x',
    upper: 'X',
    scancode: 0x2D,
};
pub const C_KEY: Key = Key {
    lower: 'c',
    upper: 'C',
    scancode: 0x2E,
};
pub const V_KEY: Key = Key {
    lower: 'v',
    upper: 'V',
    scancode: 0x2F,
};
pub const B_KEY: Key = Key {
    lower: 'b',
    upper: 'B',
    scancode: 0x30,
};
pub const N_KEY: Key = Key {
    lower: 'n',
    upper: 'N',
    scancode: 0x31,
};
pub const M_KEY: Key = Key {
    lower: 'm',
    upper: 'M',
    scancode: 0x32,
};

// Symbol keys
pub const DASH_KEY: Key = Key {
    lower: '-',
    upper: '_',
    scancode: 0x0C,
};
pub const EQUALS_KEY: Key = Key {
    lower: '=',
    upper: '+',
    scancode: 0x0D,
};
pub const LEFT_BRACKET_KEY: Key = Key {
    lower: '[',
    upper: '{',
    scancode: 0x1A,
};
pub const RIGHT_BRACKET_KEY: Key = Key {
    lower: ']',
    upper: '}',
    scancode: 0x1B,
};
pub const SEMICOLON_KEY: Key = Key {
    lower: ';',
    upper: ':',
    scancode: 0x27,
};
pub const APOSTROPHE_KEY: Key = Key {
    lower: '\'',
    upper: '"',
    scancode: 0x28,
};
pub const TILDE_KEY: Key = Key {
    lower: '`',
    upper: '~',
    scancode: 0x29,
};
pub const BACKSLASH_KEY: Key = Key {
    lower: '\\',
    upper: '|',
    scancode: 0x2B,
};
pub const COMMA_KEY: Key = Key {
    lower: ',',
    upper: '<',
    scancode: 0x33,
};
pub const DOT_KEY: Key = Key {
    lower: '.',
    upper: '>',
    scancode: 0x34,
};
pub const SLASH_KEY: Key = Key {
    lower: '/',
    upper: '?',
    scancode: 0x35,
};

// Special keys
pub const SPACE_KEY: Key = Key {
    lower: ' ',
    upper: ' ',
    scancode: 0x39,
};
pub const TAB_KEY: Key = Key {
    lower: '\t',
    upper: '\t',
    scancode: 0x0F,
};
pub const ENTER_KEY: Key = Key {
    lower: '\n',
    upper: '\n',
    scancode: 0x1C,
};
pub const BACKSPACE_KEY: Key = Key {
    lower: '\x08',
    upper: '\x08',
    scancode: 0x0E,
};

// Scancode lookup table
pub const KEYS: [Option<Key>; 128] = [
    // 0x00
    None,
    None, // Escape
    Some(ONE_KEY),
    Some(TWO_KEY),
    Some(THREE_KEY),
    Some(FOUR_KEY),
    Some(FIVE_KEY),
    Some(SIX_KEY),
    // 0x08
    Some(SEVEN_KEY),
    Some(EIGHT_KEY),
    Some(NINE_KEY),
    Some(ZERO_KEY),
    Some(DASH_KEY),
    Some(EQUALS_KEY),
    Some(BACKSPACE_KEY),
    Some(TAB_KEY),
    // 0x10
    Some(Q_KEY),
    Some(W_KEY),
    Some(E_KEY),
    Some(R_KEY),
    Some(T_KEY),
    Some(Y_KEY),
    Some(U_KEY),
    Some(I_KEY),
    // 0x18
    Some(O_KEY),
    Some(P_KEY),
    Some(LEFT_BRACKET_KEY),
    Some(RIGHT_BRACKET_KEY),
    Some(ENTER_KEY),
    None, // Left Control
    Some(A_KEY),
    Some(S_KEY),
    // 0x20
    Some(D_KEY),
    Some(F_KEY),
    Some(G_KEY),
    Some(H_KEY),
    Some(J_KEY),
    Some(K_KEY),
    Some(L_KEY),
    Some(SEMICOLON_KEY),
    // 0x28
    Some(APOSTROPHE_KEY),
    Some(TILDE_KEY),
    None, // Left Shift
    Some(BACKSLASH_KEY),
    Some(Z_KEY),
    Some(X_KEY),
    Some(C_KEY),
    Some(V_KEY),
    // 0x30
    Some(B_KEY),
    Some(N_KEY),
    Some(M_KEY),
    Some(COMMA_KEY),
    Some(DOT_KEY),
    Some(SLASH_KEY),
    None, // Right Shift
    None, // Keypad *
    // 0x38
    None, // Left Alt
    Some(SPACE_KEY),
    None, // Caps Lock
    None, // F1
    None, // F2
    None, // F3
    None, // F4
    None, // F5
    // 0x40
    None, // F6
    None, // F7
    None, // F8
    None, // F9
    None, // F10
    None, // Num Lock
    None, // Scroll Lock
    None, // Keypad 7
    // 0x48
    None, // Keypad 8
    None, // Keypad 9
    None, // Keypad -
    None, // Keypad 4
    None, // Keypad 5
    None, // Keypad 6
    None, // Keypad +
    None, // Keypad 1
    // 0x50
    None, // Keypad 2
    None, // Keypad 3
    None, // Keypad 0
    None, // Keypad .
    None,
    None,
    None,
    None, // F11
    // 0x58
    None, // F12
    None,
    None,
    None, // Left Windows/Command
    None, // Right Windows/Command
    None,
    None,
    None,
    // 0x60-0x7F
    None,
    None,
    None,
    None,
    None,
    None,
    None,
    None,
    None,
    None,
    None,
    None,
    None,
    None,
    None,
    None,
    None,
    None,
    None,
    None,
    None,
    None,
    None,
    None,
    None,
    None,
    None,
    None,
    None,
    None,
    None,
    None,
];

// Modifier key scancodes
pub const LEFT_SHIFT_PRESSED: u8 = 0x2A;
pub const LEFT_SHIFT_RELEASED: u8 = 0xAA;
pub const RIGHT_SHIFT_PRESSED: u8 = 0x36;
pub const RIGHT_SHIFT_RELEASED: u8 = 0xB6;
pub const LEFT_CTRL_PRESSED: u8 = 0x1D;
pub const LEFT_CTRL_RELEASED: u8 = 0x9D;
pub const LEFT_ALT_PRESSED: u8 = 0x38;
pub const LEFT_ALT_RELEASED: u8 = 0xB8;
pub const CAPS_LOCK_PRESSED: u8 = 0x3A;
pub const LEFT_CMD_PRESSED: u8 = 0x5B;
pub const LEFT_CMD_RELEASED: u8 = 0xDB;
pub const RIGHT_CMD_PRESSED: u8 = 0x5C;
pub const RIGHT_CMD_RELEASED: u8 = 0xDC;
