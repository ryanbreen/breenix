use lazy_static::lazy_static;
use spin::Mutex;

use core::fmt;

use crate::io::drivers::display::vga::{VGA, Color, ColorCode};

use crate::constants::vga::{BUFFER_WIDTH, BUFFER_HEIGHT};

pub struct TextBuffer {
    chars: [[u8; BUFFER_WIDTH]; BUFFER_HEIGHT],
    column_position: usize,
    color_code: ColorCode,
    active: bool,
    interactive: bool,
}

impl TextBuffer {
    pub fn activate(&mut self) {
        self.active = true;
        self.sync();
    }

    pub fn deactivate(&mut self) {
        self.active = false;
    }

    pub fn chars(&self) -> &[[u8; BUFFER_WIDTH]; BUFFER_HEIGHT] {
        &self.chars
    }

    pub fn color_code(&self) -> ColorCode {
        self.color_code
    }

    fn sync(&self) {
        if self.active {
            VGA.lock().sync_buffer(&self);
            VGA.lock().update_cursor(BUFFER_HEIGHT - 1, self.column_position);
        }
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
                self.chars[row][col] = byte;
                self.column_position += 1;
            }
        }

        if self.interactive {
            self.sync();
        }
    }

    pub fn delete_byte(&mut self) {
        if self.column_position == 0 {
            return;
        }

        let col = self.column_position - 1;

        self.chars[BUFFER_HEIGHT - 1][col] = b' ';
        self.column_position -= 1;
        self.sync();
    }

    pub fn new_line(&mut self) {
        for row in 1..BUFFER_HEIGHT {
            for col in 0..BUFFER_WIDTH {
                self.chars[row - 1][col] = self.chars[row][col];
            }
        }

        self.clear_row(BUFFER_HEIGHT - 1);
        self.column_position = 0;

        self.sync();
    }

    #[allow(dead_code)]
    pub fn clear(&mut self) {
        for _ in 0..BUFFER_HEIGHT {
            self.new_line();
        }
    }

    fn clear_row(&mut self, row: usize) {
        for col in 0..BUFFER_WIDTH {
            self.chars[row][col] = b' ';
        }
    }
}

impl ::core::fmt::Write for TextBuffer {
    fn write_str(&mut self, s: &str) -> ::core::fmt::Result {
        for byte in s.bytes() {
            self.write_byte(byte)
        }

        //serial::write(s);

        Ok(())
    }
}

pub static PRINT_BUFFER: Mutex<TextBuffer> = Mutex::new(TextBuffer {
    column_position: 0,
    color_code: ColorCode::new(Color::LightGreen, Color::Black),
    chars: [[b' '; BUFFER_WIDTH]; BUFFER_HEIGHT],
    active: true,
    interactive: false,
});

pub static KEYBOARD_BUFFER: Mutex<TextBuffer> = Mutex::new(TextBuffer {
    column_position: 0,
    color_code: ColorCode::new(Color::LightRed, Color::Black),
    chars: [[b' '; BUFFER_WIDTH]; BUFFER_HEIGHT],
    active: false,
    interactive: true,
});

pub static DEBUG_BUFFER: Mutex<TextBuffer> = Mutex::new(TextBuffer {
    column_position: 0,
    color_code: ColorCode::new(Color::LightGray, Color::Black),
    chars: [[b' '; BUFFER_WIDTH]; BUFFER_HEIGHT],
    active: false,
    interactive: false,
});

lazy_static! {
    pub static ref BUFFERS: [&'static Mutex<TextBuffer>; 3] = [&PRINT_BUFFER, &DEBUG_BUFFER, &KEYBOARD_BUFFER];
    pub static ref ACTIVE_BUFFER: Mutex<usize> = Mutex::new(0);
}

pub fn toggle() {

    let mut active = ACTIVE_BUFFER.lock();

    BUFFERS[*active].lock().deactivate();

    *active = match *active {
        2 => 0,
        _ => *active + 1
    };

    BUFFERS[*active].lock().activate();
    BUFFERS[*active].lock().sync();
}

#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => ($crate::io::drivers::display::text_buffer::print(format_args!($($arg)*)));
}

#[macro_export]
macro_rules! println {
    () => ($crate::print!("\n"));
    ($($arg:tt)*) => ($crate::print!("{}\n", format_args!($($arg)*)));
}

#[doc(hidden)]
pub fn print(args: fmt::Arguments) {
    use core::fmt::Write;
    BUFFERS[*ACTIVE_BUFFER.lock()].lock().write_fmt(args).unwrap();
}

#[test_case]
fn test_println_simple() {
    println!("test_println_simple output");
}

#[test_case]
fn test_println_many() {
    for _ in 0..200 {
        println!("test_println_many output");
    }
}

#[test_case]
fn test_println_output() {
    let s = "Some test string that fits on a single line";
    println!("{}", s);
    for (i, c) in s.chars().enumerate() {
        let screen_char = BUFFERS[*ACTIVE_BUFFER.lock()].lock().chars[BUFFER_HEIGHT - 2][i];
        assert_eq!(char::from(screen_char), c);
    }
}
