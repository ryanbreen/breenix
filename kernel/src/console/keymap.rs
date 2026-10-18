//! The console keymap: what a key event does on the foreground terminal
//! (`docs/console/contract.md` sections 1.6, 5.5 and 9.1).
//!
//! A key either acts (a switch, a scrollback view move, a lock toggle), or sends
//! the terminal its bytes in the US layout, or is ignored. Pure and
//! allocation-free: `rustc --edition=2021 --test keymap.rs` runs its tests.

#[cfg(not(test))]
use crate::input::event::*;
#[cfg(test)]
#[path = "../input/event.rs"]
mod event;
#[cfg(test)]
use event::*;

const ESC: u8 = 0x1b;

/// The chords that switch, by name, as a switch's log line gives its source.
pub const CHORDS: [&str; 14] = [
    "alt-f1", "alt-f2", "alt-f3", "alt-f4", "alt-f5", "alt-f6",
    "ctrl-alt-f1", "ctrl-alt-f2", "ctrl-alt-f3", "ctrl-alt-f4", "ctrl-alt-f5", "ctrl-alt-f6",
    "alt-left", "alt-right",
];
const ALT_F1: u8 = 0;
const CTRL_ALT_F1: u8 = 6;
const ALT_LEFT: u8 = 12;
const ALT_RIGHT: u8 = 13;

/// The bytes one key sends; the longest is Alt with a 5-byte sequence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bytes {
    len: u8,
    data: [u8; 6],
}

impl Bytes {
    fn of(bytes: &[u8]) -> Self {
        let mut data = [0; 6];
        data[..bytes.len()].copy_from_slice(bytes);
        Self { len: bytes.len() as u8, data }
    }

    fn with_escape(self) -> Self {
        let mut data = [0; 6];
        data[0] = ESC;
        data[1..=self.len as usize].copy_from_slice(self.as_slice());
        Self { len: self.len + 1, data }
    }

    pub fn as_slice(&self) -> &[u8] {
        &self.data[..self.len as usize]
    }
}

/// Which terminal a switch asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    /// Terminal 1 to 6.
    Vt(usize),
    /// The previous or next terminal, wrapping within 1 to 6.
    Previous,
    Next,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lock {
    Caps,
    Num,
    Scroll,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// No bytes and no effect: a release, a modifier, a consumed binding.
    None,
    Bytes(Bytes),
    /// Switch, naming the chord (`CHORDS`).
    Switch(Target, u8),
    /// Move the view half a screen back (true) or forward.
    Scroll(bool),
    Lock(Lock),
}

/// What the keymap needs of the foreground terminal and the console's locks.
#[derive(Clone, Copy, Debug, Default)]
pub struct State {
    /// The foreground terminal is in `KD_TEXT`.
    pub text: bool,
    /// Caps Lock on the foreground terminal.
    pub caps: bool,
    pub num: bool,
}

/// A press disposition retained by the consumer until this device releases the key.
pub fn consumes(code: u16, modifiers: Modifiers, reserved: bool, text: bool) -> bool {
    binding(code, modifiers, reserved, text).is_some()
        || matches!(code, KEY_CAPSLOCK | KEY_NUMLOCK | KEY_SCROLLLOCK)
}

/// The action of key `code` with `value` under `modifiers`. `reserved` is the
/// input core's verdict that the event belongs to a Ctrl+Alt+F1..F12 chord.
pub fn action(code: u16, value: i32, modifiers: Modifiers, reserved: bool, state: State) -> Action {
    if value == RELEASE {
        return Action::None;
    }
    if let Some(binding) = binding(code, modifiers, reserved, state.text) {
        // Only a press acts; a repeat of a binding's key has no effect.
        return if value == PRESS { binding } else { Action::None };
    }
    let lock = match code {
        KEY_CAPSLOCK => Some(Lock::Caps),
        KEY_NUMLOCK => Some(Lock::Num),
        KEY_SCROLLLOCK => Some(Lock::Scroll),
        _ => None,
    };
    if let Some(lock) = lock {
        return if value == PRESS { Action::Lock(lock) } else { Action::None };
    }
    match bytes(code, modifiers, state) {
        Some(bytes) => Action::Bytes(bytes),
        None => Action::None,
    }
}

/// The bindings of section 9.1 that act on this terminal. On a graphics
/// terminal only the reserved chords act; the others are its input.
fn binding(code: u16, modifiers: Modifiers, reserved: bool, text: bool) -> Option<Action> {
    let function = function_key(code);
    if reserved {
        return Some(match function {
            Some(n @ 1..=6) => Action::Switch(Target::Vt(n as usize), CTRL_ALT_F1 + n - 1),
            _ => Action::None,
        });
    }
    if !text || modifiers.ctrl() {
        return None;
    }
    if modifiers.alt() {
        return match (function, code) {
            (Some(n @ 1..=6), _) => Some(Action::Switch(Target::Vt(n as usize), ALT_F1 + n - 1)),
            (Some(_), _) => Some(Action::None),
            (None, KEY_LEFT) => Some(Action::Switch(Target::Previous, ALT_LEFT)),
            (None, KEY_RIGHT) => Some(Action::Switch(Target::Next, ALT_RIGHT)),
            _ => None,
        };
    }
    match code {
        KEY_PAGEUP if modifiers.shift() => Some(Action::Scroll(true)),
        KEY_PAGEDOWN if modifiers.shift() => Some(Action::Scroll(false)),
        _ => None,
    }
}

const LETTERS: [(u16, &[u8]); 3] = [(KEY_Q, b"qwertyuiop"), (KEY_A, b"asdfghjkl"), (KEY_Z, b"zxcvbnm")];
const DIGITS: &[u8] = b"1234567890";
const SHIFTED_DIGITS: &[u8] = b"!@#$%^&*()";

/// The punctuation keys without and with Shift.
fn punctuation(code: u16) -> Option<(u8, u8)> {
    Some(match code {
        KEY_MINUS => (b'-', b'_'),
        KEY_EQUAL => (b'=', b'+'),
        KEY_LEFTBRACE => (b'[', b'{'),
        KEY_RIGHTBRACE => (b']', b'}'),
        KEY_SEMICOLON => (b';', b':'),
        KEY_APOSTROPHE => (b'\'', b'"'),
        KEY_GRAVE => (b'`', b'~'),
        KEY_BACKSLASH | KEY_102ND => (b'\\', b'|'),
        KEY_COMMA => (b',', b'<'),
        KEY_DOT => (b'.', b'>'),
        KEY_SLASH => (b'/', b'?'),
        _ => return None,
    })
}

/// The sequences of section 5.5 for keys without a character.
fn sequence(code: u16) -> Option<&'static [u8]> {
    Some(match code {
        KEY_UP => b"\x1b[A",
        KEY_DOWN => b"\x1b[B",
        KEY_RIGHT => b"\x1b[C",
        KEY_LEFT => b"\x1b[D",
        KEY_HOME => b"\x1b[H",
        KEY_END => b"\x1b[F",
        KEY_INSERT => b"\x1b[2~",
        KEY_DELETE => b"\x1b[3~",
        KEY_PAGEUP => b"\x1b[5~",
        KEY_PAGEDOWN => b"\x1b[6~",
        _ => {
            const FUNCTION: [&[u8]; 12] = [
                b"\x1bOP", b"\x1bOQ", b"\x1bOR", b"\x1bOS", b"\x1b[15~", b"\x1b[17~",
                b"\x1b[18~", b"\x1b[19~", b"\x1b[20~", b"\x1b[21~", b"\x1b[23~", b"\x1b[24~",
            ];
            return function_key(code).map(|n| FUNCTION[n as usize - 1]);
        }
    })
}

/// What a keypad digit key is: its digit with Num Lock on, else the
/// navigation key it is labelled with (keypad 5 is unlabelled).
enum Keypad {
    Digit(u8),
    Navigation(u16),
    Nothing,
}

fn keypad(code: u16, num: bool) -> Option<Keypad> {
    let (digit, navigation) = match code {
        KEY_KP7 => (b'7', Some(KEY_HOME)),
        KEY_KP8 => (b'8', Some(KEY_UP)),
        KEY_KP9 => (b'9', Some(KEY_PAGEUP)),
        KEY_KP4 => (b'4', Some(KEY_LEFT)),
        KEY_KP5 => (b'5', None),
        KEY_KP6 => (b'6', Some(KEY_RIGHT)),
        KEY_KP1 => (b'1', Some(KEY_END)),
        KEY_KP2 => (b'2', Some(KEY_DOWN)),
        KEY_KP3 => (b'3', Some(KEY_PAGEDOWN)),
        KEY_KP0 => (b'0', Some(KEY_INSERT)),
        KEY_KPDOT => (b'.', Some(KEY_DELETE)),
        _ => return None,
    };
    Some(match (num, navigation) {
        (true, _) => Keypad::Digit(digit),
        (false, Some(key)) => Keypad::Navigation(key),
        (false, None) => Keypad::Nothing,
    })
}

/// The bytes key `code` sends under `modifiers`, in the US layout.
fn bytes(code: u16, modifiers: Modifiers, state: State) -> Option<Bytes> {
    let (ctrl, shift) = (modifiers.ctrl(), modifiers.shift());
    let byte = |b: u8| Some(Bytes::of(&[b]));
    let code = match keypad(code, state.num) {
        Some(Keypad::Digit(digit)) => return with_alt(byte(digit), modifiers),
        Some(Keypad::Navigation(key)) => key,
        Some(Keypad::Nothing) => return None,
        None => code,
    };
    let letter = LETTERS.iter().find_map(|&(first, row)| row.get(code.checked_sub(first)? as usize).copied());
    let bytes = if let Some(lower) = letter {
        if ctrl {
            byte(lower - b'a' + 1)
        } else if shift != state.caps {
            byte(lower.to_ascii_uppercase())
        } else {
            byte(lower)
        }
    } else if (KEY_1..=KEY_0).contains(&code) {
        let index = (code - KEY_1) as usize;
        match (ctrl, DIGITS[index]) {
            (true, b'2') => byte(0),
            (true, b'6') => byte(0x1e),
            (_, digit) => byte(if shift { SHIFTED_DIGITS[index] } else { digit }),
        }
    } else if let Some((plain, shifted)) = punctuation(code) {
        // Ctrl with [ \ ] and - gives 1B, 1C, 1D and 1F.
        match (ctrl, plain) {
            (true, b'[') => byte(0x1b),
            (true, b'\\') => byte(0x1c),
            (true, b']') => byte(0x1d),
            (true, b'-') => byte(0x1f),
            _ => byte(if shift { shifted } else { plain }),
        }
    } else {
        match code {
            KEY_SPACE => byte(if ctrl { 0 } else { b' ' }),
            KEY_ENTER | KEY_KPENTER => byte(b'\r'),
            KEY_TAB => byte(b'\t'),
            KEY_ESC => byte(ESC),
            KEY_BACKSPACE => byte(0x7f),
            KEY_KPSLASH => byte(b'/'),
            KEY_KPASTERISK => byte(b'*'),
            KEY_KPMINUS => byte(b'-'),
            KEY_KPPLUS => byte(b'+'),
            KEY_KPEQUAL => byte(b'='),
            _ => sequence(code).map(Bytes::of),
        }
    };
    with_alt(bytes, modifiers)
}

/// Alt sends ESC before a key's bytes.
fn with_alt(bytes: Option<Bytes>, modifiers: Modifiers) -> Option<Bytes> {
    bytes.map(|b| if modifiers.alt() { b.with_escape() } else { b })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mods(codes: &[u16]) -> Modifiers {
        Modifiers(codes.iter().fold(0, |held, &code| update_modifiers(held, code, PRESS)))
    }

    const TEXT: State = State { text: true, caps: false, num: false };
    const GRAPHICS: State = State { text: false, caps: false, num: false };

    fn sent(code: u16, held: &[u16], state: State) -> Vec<u8> {
        match action(code, PRESS, mods(held), false, state) {
            Action::Bytes(b) => b.as_slice().to_vec(),
            Action::None => Vec::new(),
            other => panic!("{code} sent {other:?}"),
        }
    }

    #[test]
    fn the_us_layout_types_every_printable_key() {
        let keys: Vec<u16> = (KEY_1..=KEY_BACKSPACE)
            .chain(KEY_Q..=KEY_RIGHTBRACE)
            .chain(KEY_A..=KEY_GRAVE)
            .chain(KEY_BACKSLASH..=KEY_SLASH)
            .chain([KEY_SPACE, KEY_TAB, KEY_ENTER, KEY_ESC])
            .collect();
        let typed: Vec<u8> = keys.iter().flat_map(|&k| sent(k, &[], TEXT)).collect();
        assert_eq!(typed, b"1234567890-=\x7fqwertyuiop[]asdfghjkl;'`\\zxcvbnm,./ \t\r\x1b".to_vec());
        let typed: Vec<u8> = keys.iter().flat_map(|&k| sent(k, &[KEY_RIGHTSHIFT], TEXT)).collect();
        assert_eq!(typed, b"!@#$%^&*()_+\x7fQWERTYUIOP{}ASDFGHJKL:\"~|ZXCVBNM<>? \t\r\x1b".to_vec());
        assert_eq!(sent(KEY_102ND, &[], TEXT), b"\\");
    }

    #[test]
    fn ctrl_gives_control_codes_so_ctrl_c_is_etx() {
        let c = KEY_Z + 2;
        assert_eq!(sent(c, &[KEY_LEFTCTRL], TEXT), [0x03]);
        assert_eq!(sent(c, &[KEY_RIGHTCTRL], State { caps: true, ..TEXT }), [0x03]);
        for (code, control) in [
            (KEY_A, 0x01),
            (KEY_Z, 0x1a),
            (KEY_LEFTBRACE, 0x1b),
            (KEY_BACKSLASH, 0x1c),
            (KEY_RIGHTBRACE, 0x1d),
            (KEY_1 + 5, 0x1e),
            (KEY_MINUS, 0x1f),
            (KEY_1 + 1, 0x00),
            (KEY_SPACE, 0x00),
        ] {
            assert_eq!(sent(code, &[KEY_LEFTCTRL], TEXT), [control], "{code}");
        }
    }

    #[test]
    fn caps_lock_changes_letters_only_and_shift_reverses_it() {
        let caps = State { caps: true, ..TEXT };
        assert_eq!(sent(KEY_A, &[], caps), b"A");
        assert_eq!(sent(KEY_A, &[KEY_LEFTSHIFT], caps), b"a");
        assert_eq!(sent(KEY_1, &[], caps), b"1");
        assert_eq!(action(KEY_CAPSLOCK, PRESS, mods(&[]), false, TEXT), Action::Lock(Lock::Caps));
        assert_eq!(action(KEY_CAPSLOCK, REPEAT, mods(&[]), false, TEXT), Action::None);
        assert_eq!(action(KEY_NUMLOCK, PRESS, mods(&[]), false, TEXT), Action::Lock(Lock::Num));
        assert_eq!(action(KEY_SCROLLLOCK, PRESS, mods(&[]), false, TEXT), Action::Lock(Lock::Scroll));
    }

    #[test]
    fn special_keys_send_the_contract_sequences() {
        for (code, seq) in [
            (KEY_UP, &b"\x1b[A"[..]),
            (KEY_DOWN, b"\x1b[B"),
            (KEY_RIGHT, b"\x1b[C"),
            (KEY_LEFT, b"\x1b[D"),
            (KEY_HOME, b"\x1b[H"),
            (KEY_END, b"\x1b[F"),
            (KEY_INSERT, b"\x1b[2~"),
            (KEY_DELETE, b"\x1b[3~"),
            (KEY_PAGEUP, b"\x1b[5~"),
            (KEY_PAGEDOWN, b"\x1b[6~"),
        ] {
            assert_eq!(sent(code, &[], TEXT), seq, "{code}");
        }
        let function: Vec<Vec<u8>> = (1..=12)
            .map(|n| sent(if n <= 10 { KEY_F1 + n - 1 } else { KEY_F11 + n - 11 }, &[], TEXT))
            .collect();
        let expected: Vec<&[u8]> = vec![
            b"\x1bOP", b"\x1bOQ", b"\x1bOR", b"\x1bOS", b"\x1b[15~", b"\x1b[17~",
            b"\x1b[18~", b"\x1b[19~", b"\x1b[20~", b"\x1b[21~", b"\x1b[23~", b"\x1b[24~",
        ];
        assert_eq!(function, expected);
        // A modifier alone, and a release, send no bytes.
        assert_eq!(sent(KEY_LEFTALT, &[], TEXT), b"");
        assert_eq!(action(KEY_A, RELEASE, mods(&[]), false, TEXT), Action::None);
        // A repeat of an ordinary key sends it again.
        assert_eq!(action(KEY_A, REPEAT, mods(&[]), false, TEXT), Action::Bytes(Bytes::of(b"a")));
    }

    #[test]
    fn the_keypad_follows_num_lock() {
        let num = State { num: true, ..TEXT };
        let keys = [KEY_KP1, KEY_KP2, KEY_KP3, KEY_KP4, KEY_KP5, KEY_KP6, KEY_KP7, KEY_KP8, KEY_KP9, KEY_KP0, KEY_KPDOT];
        let digits: Vec<u8> = keys.iter().flat_map(|&k| sent(k, &[], num)).collect();
        assert_eq!(digits, b"1234567890.");
        let navigation: Vec<Vec<u8>> = keys.iter().map(|&k| sent(k, &[], TEXT)).collect();
        let expected: Vec<&[u8]> = vec![
            b"\x1b[F", b"\x1b[B", b"\x1b[6~", b"\x1b[D", b"", b"\x1b[C", b"\x1b[H", b"\x1b[A", b"\x1b[5~", b"\x1b[2~",
            b"\x1b[3~",
        ];
        assert_eq!(navigation, expected);
        let operators: Vec<u8> = [KEY_KPSLASH, KEY_KPASTERISK, KEY_KPMINUS, KEY_KPPLUS, KEY_KPENTER, KEY_KPEQUAL]
            .iter()
            .flat_map(|&k| sent(k, &[], TEXT))
            .collect();
        assert_eq!(operators, b"/*-+\r=");
    }

    #[test]
    fn alt_sends_escape_first_when_the_key_has_no_console_action() {
        assert_eq!(sent(KEY_A, &[KEY_LEFTALT], TEXT), b"\x1ba");
        assert_eq!(sent(KEY_DELETE, &[KEY_RIGHTALT], TEXT), b"\x1b\x1b[3~");
        assert_eq!(sent(KEY_Z + 2, &[KEY_LEFTALT, KEY_LEFTCTRL], TEXT), b"\x1b\x03");
        assert_eq!(sent(KEY_KP1, &[KEY_LEFTALT], State { num: true, ..TEXT }), b"\x1b1");
    }

    #[test]
    fn alt_function_keys_and_arrows_switch_from_a_text_terminal() {
        for n in 1..=6u16 {
            for alt in [KEY_LEFTALT, KEY_RIGHTALT] {
                // Shift and Super do not change a binding.
                for extra in [&[][..], &[KEY_LEFTSHIFT], &[KEY_LEFTMETA]] {
                    let held: Vec<u16> = [alt].iter().chain(extra).copied().collect();
                    let a = action(KEY_F1 + n - 1, PRESS, mods(&held), false, TEXT);
                    assert_eq!(a, Action::Switch(Target::Vt(n as usize), ALT_F1 + n as u8 - 1));
                    assert_eq!(CHORDS[(ALT_F1 + n as u8 - 1) as usize], format!("alt-f{n}"));
                }
            }
        }
        let alt = mods(&[KEY_LEFTALT]);
        assert_eq!(action(KEY_LEFT, PRESS, alt, false, TEXT), Action::Switch(Target::Previous, ALT_LEFT));
        assert_eq!(action(KEY_RIGHT, PRESS, alt, false, TEXT), Action::Switch(Target::Next, ALT_RIGHT));
        assert_eq!((CHORDS[ALT_LEFT as usize], CHORDS[ALT_RIGHT as usize]), ("alt-left", "alt-right"));
        // Alt+F7 to F12 are consumed without effect; a repeat of a binding is ignored.
        for code in [KEY_F1 + 6, KEY_F10, KEY_F11, KEY_F12] {
            assert_eq!(action(code, PRESS, alt, false, TEXT), Action::None);
        }
        assert_eq!(action(KEY_F1 + 1, REPEAT, alt, false, TEXT), Action::None);
        assert_eq!(action(KEY_LEFT, REPEAT, alt, false, TEXT), Action::None);
        assert_eq!(action(KEY_F1 + 1, RELEASE, alt, false, TEXT), Action::None);
    }

    #[test]
    fn ctrl_alt_function_keys_are_the_reserved_chords_on_every_terminal() {
        let ctrl_alt = mods(&[KEY_LEFTCTRL, KEY_LEFTALT]);
        for state in [TEXT, GRAPHICS] {
            for n in 1..=6u16 {
                let a = action(KEY_F1 + n - 1, PRESS, ctrl_alt, true, state);
                assert_eq!(a, Action::Switch(Target::Vt(n as usize), CTRL_ALT_F1 + n as u8 - 1));
                assert_eq!(CHORDS[(CTRL_ALT_F1 + n as u8 - 1) as usize], format!("ctrl-alt-f{n}"));
            }
            for code in [KEY_F1 + 6, KEY_F10, KEY_F11, KEY_F12] {
                assert_eq!(action(code, PRESS, ctrl_alt, true, state), Action::None);
            }
            // The release of a reserved press, and its repeats, are ignored.
            assert_eq!(action(KEY_F1, REPEAT, ctrl_alt, true, state), Action::None);
            assert_eq!(action(KEY_F1, RELEASE, mods(&[]), true, state), Action::None);
        }
    }

    #[test]
    fn a_graphics_terminal_gets_alt_chords_as_input() {
        let alt = mods(&[KEY_LEFTALT]);
        assert_eq!(sent(KEY_F1 + 1, &[KEY_LEFTALT], GRAPHICS), b"\x1b\x1bOQ");
        assert_eq!(sent(KEY_F1 + 6, &[KEY_LEFTALT], GRAPHICS), b"\x1b\x1b[18~");
        assert_eq!(sent(KEY_LEFT, &[KEY_LEFTALT], GRAPHICS), b"\x1b\x1b[D");
        assert_eq!(sent(KEY_PAGEUP, &[KEY_LEFTSHIFT], GRAPHICS), b"\x1b[5~");
        // There, a repeat is input too.
        assert_eq!(action(KEY_F1 + 1, REPEAT, alt, false, GRAPHICS), Action::Bytes(Bytes::of(b"\x1b\x1bOQ")));
    }

    #[test]
    fn shift_page_keys_move_the_view_on_a_text_terminal() {
        let shift = mods(&[KEY_RIGHTSHIFT]);
        assert_eq!(action(KEY_PAGEUP, PRESS, shift, false, TEXT), Action::Scroll(true));
        assert_eq!(action(KEY_PAGEDOWN, PRESS, shift, false, TEXT), Action::Scroll(false));
        assert_eq!(action(KEY_PAGEUP, REPEAT, shift, false, TEXT), Action::None);
        // Without Shift they are input.
        assert_eq!(sent(KEY_PAGEUP, &[], TEXT), b"\x1b[5~");
    }
}
