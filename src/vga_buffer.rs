use core::ptr::Unique;
use spin::Mutex;

macro_rules! println {
    ($fmt:expr) => (print!(concat!($fmt, "\n")));
    ($fmt:expr, $($arg:tt)*) => (print!(concat!($fmt, "\n"), $($arg)*));
}

macro_rules! print {
  ($($arg:tt)*) => ({
    use core::fmt::Write;
    $crate::vga_buffer::PRINT_WRITER.lock().write_fmt(format_args!($($arg)*)).unwrap();
  });
}

macro_rules! debug {
  ($($arg:tt)*) => ({
    $crate::vga_buffer::debug();
  });
}

#[repr(u8)]
#[allow(dead_code)]
pub enum Color {
  Black      = 0,
  Blue       = 1,
  Green      = 2,
  Cyan       = 3,
  Red        = 4,
  Magenta    = 5,
  Brown      = 6,
  LightGray  = 7,
  DarkGray   = 8,
  LightBlue  = 9,
  LightGreen = 10,
  LightCyan  = 11,
  LightRed   = 12,
  Pink       = 13,
  Yellow     = 14,
  White      = 15,
}

#[derive(Clone, Copy)]
struct ColorCode(u8);

impl ColorCode {
  const fn new(foreground: Color, background: Color) -> ColorCode {
    ColorCode((background as u8) << 4 | (foreground as u8))
  }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ScreenChar {
  ascii_character: u8,
  color_code: ColorCode,
}

const BUFFER_HEIGHT: usize = 25;
const BUFFER_WIDTH: usize = 80;

struct Buffer {
  chars: [[ScreenChar; BUFFER_WIDTH]; BUFFER_HEIGHT],
}

pub struct Writer {
  column_position: usize,
  color_code: ColorCode,
  buffer: Unique<Buffer>,
  shadow_buffer: Buffer,
  active: bool,
}

impl Writer {
  pub fn activate(&mut self) {
    self.active = true;

    // Write shadow buffer to screen
    for row in 0..BUFFER_HEIGHT {
      let buffer = unsafe { self.buffer.get_mut() };
      buffer.chars[row] = self.shadow_buffer.chars[row];
    }
  }

  pub fn deactivate(&mut self) {
    self.active = false;
  }

  pub fn write_byte(&mut self, byte: u8) {
    match byte {
      b'\n' => self.new_line(),
      byte => {
        if self.column_position >= BUFFER_WIDTH {
          self.new_line();
        }

        let row = BUFFER_HEIGHT - 1;
        let col = self.column_position;

        let cc = self.color_code;

        self.write_to_buffers(row, col, ScreenChar {
          ascii_character: byte,
          color_code: cc,
        });
        self.column_position += 1;
      }
    }
  }

  pub fn delete_byte(&mut self) {
    if self.column_position == 0 {
      return;
    }

    let col = self.column_position-1;
    let cc = self.color_code;
    self.write_to_buffers(BUFFER_HEIGHT-1, col, ScreenChar {
      ascii_character: b' ',
      color_code: cc,
    });
    self.column_position -= 1;

    if self.active {
      update_cursor(BUFFER_HEIGHT as u8 -1, self.column_position as u8);
    }
  }

  fn write_to_buffers(&mut self, row: usize, col: usize, sc:ScreenChar) {
    if self.active {
      unsafe{ self.buffer.get_mut().chars[row][col] = sc; }

      if col < BUFFER_WIDTH - 1 {
        update_cursor(row as u8, (col + 1) as u8);
      }
    }

    self.shadow_buffer.chars[row][col] = sc;
  }

  pub fn new_line(&mut self) {
    for row in 0..(BUFFER_HEIGHT-1) {
      if self.active {
        let buffer = unsafe { self.buffer.get_mut() };
        buffer.chars[row] = buffer.chars[row + 1];
      }

      self.shadow_buffer.chars[row] = self.shadow_buffer.chars[row + 1];
    }
    self.clear_row(BUFFER_HEIGHT-1);
    self.column_position = 0;

    if self.active {
      update_cursor(BUFFER_HEIGHT as u8 -1, 0);
    }
  }

  fn clear_row(&mut self, row: usize) {
    if self.active {
      unsafe{
        self.buffer.get_mut().chars[row] = [ScreenChar {
          ascii_character: b' ',
          color_code: self.color_code,
        }; BUFFER_WIDTH];
      }
    }

    self.shadow_buffer.chars[row] = [ScreenChar {
      ascii_character: b' ',
      color_code: self.color_code,
    }; BUFFER_WIDTH];
  }

  pub fn clear(&mut self) {
    for row in 0..BUFFER_HEIGHT {
      self.clear_row(row);
    }
  }
}

impl ::core::fmt::Write for Writer {
  fn write_str(&mut self, s: &str) -> ::core::fmt::Result {
    for byte in s.bytes() {
      self.write_byte(byte)
    }

    Ok(())
  }
}

const GREEN_BLANK:ScreenChar = ScreenChar {
  ascii_character: b' ',
  color_code: ColorCode::new(Color::LightGreen, Color::Black),
};

const RED_BLANK:ScreenChar = ScreenChar {
  ascii_character: b' ',
  color_code: ColorCode::new(Color::LightRed, Color::Black),
};

const GRAY_BLANK:ScreenChar = ScreenChar {
  ascii_character: b' ',
  color_code: ColorCode::new(Color::LightGray, Color::Black),
};

pub static PRINT_WRITER: Mutex<Writer> = Mutex::new(Writer {
  column_position: 0,
  color_code: ColorCode::new(Color::LightGreen, Color::Black),
  buffer: unsafe { Unique::new(0xb8000 as *mut _) },
  shadow_buffer: Buffer {
    chars: [[GREEN_BLANK; BUFFER_WIDTH]; BUFFER_HEIGHT]
  },
  active: true,
});

pub static KEYBOARD_WRITER: Mutex<Writer> = Mutex::new(Writer {
  column_position: 0,
  color_code: ColorCode::new(Color::LightRed, Color::Black),
  buffer: unsafe { Unique::new(0xb8000 as *mut _) },
  shadow_buffer: Buffer {
    chars: [[RED_BLANK; BUFFER_WIDTH]; BUFFER_HEIGHT]
  },
  active: false,
});

pub static DEBUG_WRITER: Mutex<Writer> = Mutex::new(Writer {
  column_position: 0,
  color_code: ColorCode::new(Color::LightGray, Color::Black),
  buffer: unsafe { Unique::new(0xb8000 as *mut _) },
  shadow_buffer: Buffer {
    chars: [[GRAY_BLANK; BUFFER_WIDTH]; BUFFER_HEIGHT]
  },
  active: false,
});

#[allow(exceeding_bitshifts)]
pub fn update_cursor(row: u8, col: u8) {
  let position:u16 = (row as u16 * (BUFFER_WIDTH as u16)) + col as u16;
  use io::Port;

  unsafe {
    let mut cursor_control_port:Port<u8> = Port::new(0x3D4);
    let mut cursor_value_port:Port<u8> = Port::new(0x3D5);

    // cursor HIGH port to vga INDEX register
    cursor_control_port.write(0x0E);
    cursor_value_port.write(((position>>8)&0xFF) as u8);
    // cursor LOW port to vga INDEX register
    cursor_control_port.write(0x0F);
    cursor_value_port.write((position&0xFF) as u8);
  }
}

#[allow(unused_must_use)]
pub fn debug() {
  use core::fmt::Write;
  use x86::controlregs::{cr0, cr2, cr3, cr4};
  use x86::msr::{IA32_EFER, MSR_EBC_FREQUENCY_ID};
  use x86::msr::rdmsr;
  use x86::perfcnt;

  let mut writer = DEBUG_WRITER.lock();
  writer.clear();
  unsafe {
    writer.write_fmt(format_args!("cr0: 0x{:x}\n", cr0()));
    writer.write_fmt(format_args!("cr2: 0x{:x}\n", cr2()));
    writer.write_fmt(format_args!("cr3: 0x{:x}\n", cr3()));
    writer.write_fmt(format_args!("cr4: 0x{:x}\n", cr4()));

    writer.write_fmt(format_args!("msr IA32_EFER: 0x{:x}\n", rdmsr(IA32_EFER)));

    /*
    perfcnt.core_counters().map(|cc| {

    });
    */
  }
}

pub fn clear_screen() {
  unsafe { ACTIVE_WRITER.lock().clear(); }
}

static mut ACTIVE_WRITER:&'static Mutex<Writer> = &PRINT_WRITER;
static mut INACTIVE_WRITERS:[&'static Mutex<Writer>;2] = [&KEYBOARD_WRITER, &DEBUG_WRITER];

pub fn toggle() {
  unsafe {
    ACTIVE_WRITER.lock().deactivate();

    let new_active = INACTIVE_WRITERS[0];
    INACTIVE_WRITERS[0] = INACTIVE_WRITERS[1];
    INACTIVE_WRITERS[1] = ACTIVE_WRITER;
    ACTIVE_WRITER = new_active;
    ACTIVE_WRITER.lock().activate();

    update_cursor(BUFFER_HEIGHT as u8 -1, ACTIVE_WRITER.lock().column_position as u8);
  }
}
