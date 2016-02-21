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

pub fn scancode_to_ascii(code: u8) -> char {
  match code {
    ENTER_PRESSED => return '\n',
    SPACE_PRESSED => return ' ',
    POINT_RELEASED => return '.',
    SLASH_RELEASED => return '/',
    ZERO_PRESSED => return '0',
    _ => {
      if code >= ONE_PRESSED && code <= NINE_PRESSED {
        return NUM[(code - ONE_PRESSED) as usize];
      }
      if code >= 0x10 && code <= 0x1C {
        return QUERTYZUIOP[(code - 0x10) as usize];
      }
      if code >= 0x1E && code <= 0x26 {
        return ASDFGHJKL[(code - 0x1E) as usize];
      }
      if code >= 0x2C && code <= 0x32 {
        return YXCVBNM[(code - 0x2C) as usize];
      }
      return ' ';
    },
  }
}

pub fn test() {

  loop {
    unsafe {

      let scancode = KEYBOARD.lock().read();

      // If the user hits 'q', exit.
      if scancode == 16 {
        io::PICS.lock().initialize();
        println!("Interrupts engaged");
        panic!();
      }

      if scancode != 0xFA {
        print!("{}", scancode_to_ascii(scancode));
      }

    }
  }
}