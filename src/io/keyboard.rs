use spin::Mutex;

use io;
use io::Port;

static KEYBOARD: Mutex<Port<u8>> = Mutex::new(unsafe {
  Port::new(0x60)
});

#[derive(Debug, Clone, Copy)]
pub struct Key {
  lower: char,
  upper: char,
  scancode: u8
}

const ONE_KEY:Key = Key { lower:'1', upper:'!', scancode: 0x2 };
const TWO_KEY:Key = Key { lower:'2', upper:'@', scancode: 0x3 };
const THREE_KEY:Key = Key { lower:'3', upper:'#', scancode: 0x4 };
const FOUR_KEY:Key = Key { lower:'4', upper:'$', scancode: 0x5 };
const FIVE_KEY:Key = Key { lower:'5', upper:'%', scancode: 0x6 };
const SIX_KEY:Key = Key { lower:'6', upper:'^', scancode: 0x7 };
const SEVEN_KEY:Key = Key { lower:'7', upper:'&', scancode: 0x8 };
const EIGHT_KEY:Key = Key { lower:'8', upper:'*', scancode: 0x9 };
const NINE_KEY:Key = Key { lower:'9', upper:'(', scancode: 0xA };

static KEYS:[Option<Key>;256] = [
  /* 0x0   */ None, None, Some(ONE_KEY), Some(TWO_KEY), Some(THREE_KEY), Some(FOUR_KEY), Some(FIVE_KEY), Some(SIX_KEY), /*0x7 */
  /* 0x8   */ Some(SEVEN_KEY), Some(EIGHT_KEY), Some(NINE_KEY), None, None, None, None, None, /* 0xF */
  /* 0x10  */ None, None, None, None, None, None, None, None, None, None, None, None, None, None, None, None, /* 0x1F */
  /* 0x20  */ None, None, None, None, None, None, None, None, None, None, None, None, None, None, None, None, /* 0x2F */
  /* 0x30  */ None, None, None, None, None, None, None, None, None, None, None, None, None, None, None, None, /* 0x3F */
  /* 0x40  */ None, None, None, None, None, None, None, None, None, None, None, None, None, None, None, None, /* 0x4F */
  /* 0x50  */ None, None, None, None, None, None, None, None, None, None, None, None, None, None, None, None, /* 0x5F */
  /* 0x60  */ None, None, None, None, None, None, None, None, None, None, None, None, None, None, None, None, /* 0x6F */
  /* 0x70  */ None, None, None, None, None, None, None, None, None, None, None, None, None, None, None, None, /* 0x7F */
  /* 0x80  */ None, None, None, None, None, None, None, None, None, None, None, None, None, None, None, None, /* 0x8F */
  /* 0x90  */ None, None, None, None, None, None, None, None, None, None, None, None, None, None, None, None, /* 0x9F */
  /* 0x100 */ None, None, None, None, None, None, None, None, None, None, None, None, None, None, None, None, /* 0x10F */
  /* 0x110 */ None, None, None, None, None, None, None, None, None, None, None, None, None, None, None, None, /* 0x11F */
  /* 0x120 */ None, None, None, None, None, None, None, None, None, None, None, None, None, None, None, None, /* 0x12F */
  /* 0x130 */ None, None, None, None, None, None, None, None, None, None, None, None, None, None, None, None, /* 0x13F */
  /* 0x140 */ None, None, None, None, None, None, None, None, None, None, None, None, None, None, None, None, /* 0x14F */
  /* 0x140 */ None, None, None, None, None, None, None, None, None, None, None, None, None, None, None, None, /* 0x15F */
];

const ZERO_PRESSED:u8 = 0x29;

const POINT_PRESSED:u8 = 0x34;
const POINT_RELEASED:u8 = 0xB4;

const SLASH_RELEASED:u8 = 0xB5;

const BACKSPACE_PRESSED:u8 = 0xE;
const BACKSPACE_RELEASED:u8 = 0x8E;
const SPACE_PRESSED:u8 = 0x39;
const SPACE_RELEASED:u8 = 0xB9;
const ENTER_PRESSED:u8 = 0x1C;
const ENTER_RELEASED:u8 = 0x9C;

static QUERTYUIOP: [char;10] = ['q','w','e','r','t','y','u','i','o','p']; // 0x10-0x1c
static ASDFGHJKL: [char;9] = ['a','s','d','f','g','h','j','k','l'];
static ZXCVBNM: [char;7] = ['z','x','c','v','b','n','m'];
static NUM: [char;9] = ['1','2','3','4','5','6','7','8','9'];

pub fn scancode_to_key(code: u8) -> Option<Key> {
  return KEYS[code as usize];
/*  match code {
    ENTER_PRESSED => return Some('\n'),
    SPACE_PRESSED => return Some(' '),
    POINT_RELEASED => return Some('.'),
    SLASH_RELEASED => return Some('/'),
    ZERO_PRESSED => return Some('0'),
    _ => {
      if code >= ONE_PRESSED && code <= NINE_PRESSED {
        return Some(NUM[(code - ONE_PRESSED) as usize]);
      }
      if code >= 0x10 && code <= 0x1C {
        return Some(QUERTYUIOP[(code - 0x10) as usize]);
      }
      if code >= 0x1E && code <= 0x26 {
        return Some(ASDFGHJKL[(code - 0x1E) as usize]);
      }
      if code >= 0x2C && code <= 0x32 {
        return Some(ZXCVBNM[(code - 0x2C) as usize]);
      }
      return None;
    },
  }
  */
}

/// Our keyboard state, including our I/O port, our currently pressed
/// modifiers, etc.
struct State {
  /// The PS/2 serial IO port for the keyboard.  There's a huge amount of
  /// emulation going on at the hardware level to allow us to pretend to
  /// be an early-80s IBM PC.
  ///
  /// We could read the standard keyboard port directly using
  /// `inb(0x60)`, but it's nicer if we wrap it up in a `Port` object.
  port: Port<u8>,

  /// The collection of currently-pressed modifier keys.
  modifiers: Modifiers,
}

struct Modifiers {
  l_shift: bool,
  r_shift: bool,
  caps_lock: bool,
}

impl Modifiers {
  const fn new() -> Modifiers {
    Modifiers {
      l_shift: false,
      r_shift: false,
      caps_lock: false,
    }
  }

  fn update(&mut self, scancode: u8) {
    match scancode {
      0x2A => self.l_shift = true,
      0xAA => self.l_shift = false,
      0x36 => self.r_shift = true,
      0xB6 => self.l_shift = false,
      0x3A => self.caps_lock = !self.caps_lock,
      _ => {},
    }
  }

  fn apply_to(&self, key: Key) -> char {
    if (self.l_shift || self.r_shift) ^ self.caps_lock {
      return key.upper;
    }

    return key.lower;
  }
}

/// Our global keyboard state, protected by a mutex.
static STATE: Mutex<State> = Mutex::new(State {
    port: unsafe { Port::new(0x60) },
    modifiers: Modifiers::new(),
});

/// Try to read a single input character
pub fn read_char() -> Option<char> {
  let mut state = STATE.lock();

  // Read a single scancode off our keyboard port.
  let scancode = unsafe { state.port.read() };

  // Give our modifiers first crack at this.
  state.modifiers.update(scancode);

  // Look up the ASCII keycode.
  if let Some(key) = scancode_to_key(scancode) {
      // The `as char` converts our ASCII data to Unicode, which is
      // correct as long as we're only using 7-bit ASCII.
      return Some(state.modifiers.apply_to(key))
  } else {
      // Either this was a modifier key, or it some key we don't know how
      // to handle yet, or it's part of a multibyte scancode.  Just look
      // innocent and pretend nothing happened.
      return None;
  }
}