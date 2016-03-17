use spin::Mutex;

use io::Port;
use io::interrupts;

use buffers;
use buffers::KEYBOARD_BUFFER;

use state;

use event;
use event::EventType;
use event::IsEvent;
use event::IsListener;

/// Event framework

#[derive(Clone, Copy)]
pub struct ControlKeyState {
  ctrl: bool,
  alt: bool,
  shift: bool,
  caps_lock: bool,
  scroll_lock: bool,
  num_lock: bool
}

#[derive(Clone, Copy)]
pub struct KeyEvent {
  event_type: EventType,
  scancode: u8,
  character: char,
  controls: ControlKeyState
}

impl IsEvent for KeyEvent {
  fn event_type(&self) -> EventType {
    self.event_type
  }
}

impl KeyEvent {
  const fn new(scancode: u8, character: char, modifiers: &Modifiers) -> KeyEvent {
    KeyEvent {
      event_type: EventType::KeyEvent,
      scancode: scancode,
      character: character,
      controls: ControlKeyState {
        ctrl: modifiers.l_cmd || modifiers.r_cmd,
        alt: modifiers.l_alt || modifiers.r_alt,
        shift: modifiers.l_shift || modifiers.r_shift,
        caps_lock: modifiers.caps_lock,
        scroll_lock: false,
        num_lock: false,
      }
    }
  }
}

#[derive(Debug, Clone, Copy)]
struct Key {
  lower: char,
  upper: char,
  scancode: u8
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

#[allow(dead_code)]
struct Modifiers {
  l_shift: bool,
  r_shift: bool,
  caps_lock: bool,
  l_cmd: bool,
  r_cmd: bool,
  l_alt: bool,
  r_alt: bool,
  last_key: u8,
}

impl Modifiers {
  const fn new() -> Modifiers {
    Modifiers {
      l_shift: false,
      r_shift: false,
      caps_lock: false,
      l_cmd: false,
      r_cmd: false,
      l_alt: false,
      r_alt: false,
      last_key: 0,
    }
  }

  fn cmd(&self) -> bool {
    self.l_cmd || self.r_cmd
  }

  fn update(&mut self, scancode: u8) {

    //println!("{:x} {:x}", self.last_key, scancode);

    if self.last_key == 0xE0 {
      match scancode {
        0x5B => self.l_cmd = true,
        0xDB => self.l_cmd = false,
        0x5C => self.r_cmd = true,
        0xDC => self.r_cmd = false,
        _ => {},
      }
    } else {
      match scancode {
        0x2A => self.l_shift = true,
        0xAA => self.l_shift = false,
        0x36 => self.r_shift = true,
        0xB6 => self.r_shift = false,
        0x3A => self.caps_lock = !self.caps_lock,
        _ => {},
      }
    }

    self.last_key = scancode;
  }

  fn apply_to(&self, key: Key) -> Option<char> {

    // Only alphabetic keys honor caps lock, so first distinguish between
    // alphabetic and non alphabetic keys.
    if (0x10 <= key.scancode && key.scancode <= 0x19) ||
       (0x1E <= key.scancode && key.scancode <= 0x26) ||
       (0x2C <= key.scancode && key.scancode <= 0x32) {
      if (self.l_shift || self.r_shift) ^ self.caps_lock {
        return Some(key.upper);
      }
    } else {
      if self.l_shift || self.r_shift {
        return Some(key.upper);
      }
    }

    return Some(key.lower);
  }
}

/// Our global keyboard state, protected by a mutex.
static STATE: Mutex<State> = Mutex::new(State {
  port: unsafe { Port::new(0x60) },
  modifiers: Modifiers::new(),
});

/// Try to read a single input character
pub fn read() {
  let mut state = STATE.lock();

  // Read a single scancode off our keyboard port.
  let scancode:u8 = state.port.read();

  //println!("{:x}", scancode);

  // Give our modifiers first crack at this.
  state.modifiers.update(scancode);

  // We don't map any keys > 127.
  if scancode > 127 {
    return;
  }

  // Look up the ASCII keycode.
  if let Some(key) = KEYS[scancode as usize] {
    // The `as char` converts our ASCII data to Unicode, which is
    // correct as long as we're only using 7-bit ASCII.
    if let Some(transformed_ascii) = state.modifiers.apply_to(key) {
      state::dispatch_key_event(&KeyEvent::new(scancode, transformed_ascii, &state.modifiers));
      return;
    }
  }

  state::dispatch_key_event(&KeyEvent::new(scancode, 0 as char, &state.modifiers));
}

pub struct KeyEventScreenWriter {}

impl IsListener<KeyEvent> for KeyEventScreenWriter {
  fn handles_event(&self, ev: &KeyEvent) -> bool {
    !(ev.scancode == S_KEY.scancode && ev.controls.ctrl)
  }

  fn notify(&self, ev: &KeyEvent) {

    if ev.scancode == ENTER_KEY.scancode {
      KEYBOARD_BUFFER.lock().new_line();
      return;
    }

    if ev.scancode == DELETE_KEY.scancode {
      KEYBOARD_BUFFER.lock().delete_byte();
      return;
    }

    if ev.character as u8 != 0 {
      KEYBOARD_BUFFER.lock().write_byte(ev.character as u8);
    }
    
  }
}

pub struct ToggleWatcher{}

impl IsListener<KeyEvent> for ToggleWatcher {
  fn handles_event(&self, ev: &KeyEvent) -> bool {
    ev.scancode == S_KEY.scancode && ev.controls.ctrl
  }

  fn notify(&self, ev: &KeyEvent) {
    // Switch buffers
    buffers::toggle();
  }
}

/// Super Boring Scancode Mappings below!

const ZERO_KEY:Key = Key { lower:'0', upper:')', scancode: 0x29 };
const ONE_KEY:Key = Key { lower:'1', upper:'!', scancode: 0x2 };
const TWO_KEY:Key = Key { lower:'2', upper:'@', scancode: 0x3 };
const THREE_KEY:Key = Key { lower:'3', upper:'#', scancode: 0x4 };
const FOUR_KEY:Key = Key { lower:'4', upper:'$', scancode: 0x5 };
const FIVE_KEY:Key = Key { lower:'5', upper:'%', scancode: 0x6 };
const SIX_KEY:Key = Key { lower:'6', upper:'^', scancode: 0x7 };
const SEVEN_KEY:Key = Key { lower:'7', upper:'&', scancode: 0x8 };
const EIGHT_KEY:Key = Key { lower:'8', upper:'*', scancode: 0x9 };
const NINE_KEY:Key = Key { lower:'9', upper:'(', scancode: 0xA };
const DASH_KEY:Key = Key { lower:'-', upper:'_', scancode: 0xC };
const EQUAL_KEY:Key = Key { lower: '=', upper:'+', scancode: 0xD };
const DELETE_KEY:Key = Key { lower: ' ', upper:' ', scancode: 0xE };
const TAB_KEY:Key = Key { lower:'\t', upper:'\t', scancode: 0xF };
const Q_KEY:Key = Key { lower:'q', upper:'Q', scancode: 0x10 };
const W_KEY:Key = Key { lower:'w', upper:'W', scancode: 0x11 };
const E_KEY:Key = Key { lower:'e', upper:'E', scancode: 0x12 };
const R_KEY:Key = Key { lower:'r', upper:'R', scancode: 0x13 };
const T_KEY:Key = Key { lower:'t', upper:'T', scancode: 0x14 };
const Y_KEY:Key = Key { lower:'y', upper:'Y', scancode: 0x15 };
const U_KEY:Key = Key { lower:'u', upper:'U', scancode: 0x16 };
const I_KEY:Key = Key { lower:'i', upper:'I', scancode: 0x17 };
const O_KEY:Key = Key { lower:'o', upper:'O', scancode: 0x18 };
const P_KEY:Key = Key { lower:'p', upper:'P', scancode: 0x19 };
const LB_KEY:Key = Key { lower:'[', upper:'{', scancode: 0x1A };
const RB_KEY:Key = Key { lower:']', upper:'}', scancode: 0x1B };
const ENTER_KEY:Key = Key { lower:'\r', upper:'\r', scancode: 0x1C };
const A_KEY:Key = Key { lower:'a', upper:'A', scancode: 0x1E };
const S_KEY:Key = Key { lower:'s', upper:'S', scancode: 0x1F };
const D_KEY:Key = Key { lower:'d', upper:'D', scancode: 0x20 };
const F_KEY:Key = Key { lower:'f', upper:'F', scancode: 0x21 };
const G_KEY:Key = Key { lower:'g', upper:'G', scancode: 0x22 };
const H_KEY:Key = Key { lower:'h', upper:'H', scancode: 0x23 };
const J_KEY:Key = Key { lower:'j', upper:'J', scancode: 0x24 };
const K_KEY:Key = Key { lower:'k', upper:'K', scancode: 0x25 };
const L_KEY:Key = Key { lower:'l', upper:'L', scancode: 0x26 };
const TILDE_KEY:Key = Key { lower:'`', upper:'~', scancode: 0x29 };
const BACKSLASH_KEY:Key = Key { lower:'\\', upper:'|', scancode: 0x2B };
const Z_KEY:Key = Key { lower:'z', upper:'Z', scancode: 0x2C };
const X_KEY:Key = Key { lower:'x', upper:'X', scancode: 0x2D };
const C_KEY:Key = Key { lower:'c', upper:'C', scancode: 0x2E };
const V_KEY:Key = Key { lower:'v', upper:'V', scancode: 0x2F };
const B_KEY:Key = Key { lower:'b', upper:'B', scancode: 0x30 };
const N_KEY:Key = Key { lower:'n', upper:'N', scancode: 0x31 };
const M_KEY:Key = Key { lower:'m', upper:'M', scancode: 0x32 };
const COMMA_KEY:Key = Key { lower:',', upper:'<', scancode: 0x33 };
const DOT_KEY:Key = Key { lower:'.', upper:'>', scancode: 0x34 };
const SLASH_KEY:Key = Key { lower:'/', upper:'?', scancode: 0x35 };
const SPACE_KEY:Key = Key { lower:' ', upper:' ', scancode: 0x39 };

static KEYS:[Option<Key>;128] = [
  /* 0x0   */ None, None, Some(ONE_KEY), Some(TWO_KEY), Some(THREE_KEY), Some(FOUR_KEY), Some(FIVE_KEY), Some(SIX_KEY), /*0x7 */
  /* 0x8   */ Some(SEVEN_KEY), Some(EIGHT_KEY), Some(NINE_KEY), Some(ZERO_KEY), Some(DASH_KEY),
                                              Some(EQUAL_KEY), Some(DELETE_KEY), Some(TAB_KEY), /* 0xF */
  /* 0x10  */ Some(Q_KEY), Some(W_KEY), Some(E_KEY), Some(R_KEY), Some(T_KEY), Some(Y_KEY), Some(U_KEY), Some(I_KEY), /* 0x17 */
  /* 0x18  */ Some(O_KEY), Some(P_KEY), Some(LB_KEY), Some(RB_KEY), Some(ENTER_KEY), None, Some(A_KEY), Some(S_KEY), /* 0x1F */
  /* 0x20  */ Some(D_KEY), Some(F_KEY), Some(G_KEY), Some(H_KEY), Some(J_KEY), Some(K_KEY), Some(L_KEY), None, /* 0x27 */
  /* 0x28  */ None, Some(TILDE_KEY), None, Some(BACKSLASH_KEY), Some(Z_KEY), Some(X_KEY), Some(C_KEY), Some(V_KEY), /* 0x2F */
  /* 0x30  */ Some(B_KEY), Some(N_KEY), Some(M_KEY), Some(COMMA_KEY), Some(DOT_KEY), Some(SLASH_KEY), None, None, /* 0x37 */
  /* 0x38  */ None, Some(SPACE_KEY), None, None, None, None, None, None, /* 0x3F */
  /* 0x40  */ None, None, None, None, None, None, None, None, /* 0x47 */
  /* 0x48  */ None, None, None, None, None, None, None, None, /* 0x4F */
  /* 0x50  */ None, None, None, None, None, None, None, None, /* 0x57 */
  /* 0x58  */ None, None, None, None, None, None, None, None, /* 0x5F */
  /* 0x60  */ None, None, None, None, None, None, None, None, /* 0x67 */
  /* 0x68  */ None, None, None, None, None, None, None, None, /* 0x6F */
  /* 0x70  */ None, None, None, None, None, None, None, None, /* 0x77 */
  /* 0x78  */ None, None, None, None, None, None, None, None, /* 0x7F */
];
