use spin::Mutex;

use io;
use io::Port;

static KEYBOARD: Mutex<Port<u8>> = Mutex::new(unsafe {
  Port::new(0x60)
});

const ZERO_PRESSED:u8 = 0x29;
const ONE_PRESSED:u8 = 0x2;
const NINE_PRESSED:u8 = 0xA;

const POINT_PRESSED:u8 = 0x34;
const POINT_RELEASED:u8 = 0xB4;

const SLASH_RELEASED:u8 = 0xB5;

const BACKSPACE_PRESSED:u8 = 0xE;
const BACKSPACE_RELEASED:u8 = 0x8E;
const SPACE_PRESSED:u8 = 0x39;
const SPACE_RELEASED:u8 = 0xB9;
const ENTER_PRESSED:u8 = 0x1C;
const ENTER_RELEASED:u8 = 0x9C;

static QUERTYZUIOP: [char;10] = ['q','w','e','r','t','z','u','i','o','p']; // 0x10-0x1c
static ASDFGHJKL: [char;9] = ['a','s','d','f','g','h','j','k','l'];
static YXCVBNM: [char;7] = ['y','x','c','v','b','n','m'];
static NUM: [char;9] = ['1','2','3','4','5','6','7','8','9'];

pub fn scancode_to_ascii(code: u8) -> Option<char> {
  match code {
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
        return Some(QUERTYZUIOP[(code - 0x10) as usize]);
      }
      if code >= 0x1E && code <= 0x26 {
        return Some(ASDFGHJKL[(code - 0x1E) as usize]);
      }
      if code >= 0x2C && code <= 0x32 {
        return Some(YXCVBNM[(code - 0x2C) as usize]);
      }
      return None;
    },
  }
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

  fn apply_to(&self, ascii: char) -> char {
    if (self.l_shift || self.r_shift) ^ self.caps_lock {
      return ((ascii as u8) - 32) as char;
    }

    return ascii;
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
  if let Some(ascii) = scancode_to_ascii(scancode) {
      // The `as char` converts our ASCII data to Unicode, which is
      // correct as long as we're only using 7-bit ASCII.
      return Some(state.modifiers.apply_to(ascii))
  } else {
      // Either this was a modifier key, or it some key we don't know how
      // to handle yet, or it's part of a multibyte scancode.  Just look
      // innocent and pretend nothing happened.
      return None;
  }
}