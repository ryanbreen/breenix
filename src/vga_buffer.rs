use core::ptr::Unique;
use core::fmt::Write;
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
    let col = self.column_position-1;
    self.write_to_buffers(BUFFER_HEIGHT-1, col, BLANK);
    self.column_position -= 1;
  }

  fn write_to_buffers(&mut self, row: usize, col: usize, sc:ScreenChar) {
    if self.active {
      unsafe{ self.buffer.get_mut().chars[row][col] = sc; }
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
  }

  fn clear_row(&mut self, row: usize) {
    if self.active {
      unsafe{ self.buffer.get_mut().chars[row] = [BLANK; BUFFER_WIDTH]; }
    }

    self.shadow_buffer.chars[row] = [BLANK; BUFFER_WIDTH];
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

const BLANK:ScreenChar = ScreenChar {
  ascii_character: b' ',
  color_code: ColorCode::new(Color::LightGreen, Color::Black),
};

pub static PRINT_WRITER: Mutex<Writer> = Mutex::new(Writer {
  column_position: 0,
  color_code: ColorCode::new(Color::LightGreen, Color::Black),
  buffer: unsafe { Unique::new(0xb8000 as *mut _) },
  shadow_buffer: Buffer {
    chars: [[BLANK; BUFFER_WIDTH]; BUFFER_HEIGHT]
  },
  active: true,
});

pub static KEYBOARD_WRITER: Mutex<Writer> = Mutex::new(Writer {
  column_position: 0,
  color_code: ColorCode::new(Color::LightRed, Color::Black),
  buffer: unsafe { Unique::new(0xb8000 as *mut _) },
  shadow_buffer: Buffer {
    chars: [[BLANK; BUFFER_WIDTH]; BUFFER_HEIGHT]
  },
  active: false,
});

pub fn clear_screen() {
  for _ in 0..BUFFER_HEIGHT {
    println!("");
  }
}

static mut ACTIVE_WRITER:&'static Mutex<Writer> = &PRINT_WRITER;
static mut INACTIVE_WRITER:&'static Mutex<Writer> = &KEYBOARD_WRITER;

pub fn toggle() {
  unsafe {
    ACTIVE_WRITER.lock().deactivate();
    INACTIVE_WRITER.lock().activate();

    let new_active = INACTIVE_WRITER;
    INACTIVE_WRITER = ACTIVE_WRITER;
    ACTIVE_WRITER = new_active;
  }
}
