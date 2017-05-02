use constants::vga::{BUFFER_WIDTH, BUFFER_HEIGHT};
use constants::vga::{GREEN_BLANK, RED_BLANK};

use core::fmt;
use core::ptr::Unique;

use spin::Mutex;

use volatile::Volatile;

#[repr(u8)]
#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
pub enum Color {
    Black = 0,
    Blue = 1,
    Green = 2,
    Cyan = 3,
    Red = 4,
    Magenta = 5,
    Brown = 6,
    LightGray = 7,
    DarkGray = 8,
    LightBlue = 9,
    LightGreen = 10,
    LightCyan = 11,
    LightRed = 12,
    Pink = 13,
    Yellow = 14,
    White = 15,
}

#[derive(Debug, Clone, Copy)]
pub struct ColorCode(u8);

impl ColorCode {
    pub const fn new(foreground: Color, background: Color) -> ColorCode {
        ColorCode((background as u8) << 4 | (foreground as u8))
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct ScreenChar {
    pub ascii_character: u8,
    pub color_code: ColorCode,
}

struct ScreenBuffer {
    chars: [[Volatile<ScreenChar>; BUFFER_WIDTH]; BUFFER_HEIGHT],
}

pub struct VGA {
    column_position: usize,
    color_code: ColorCode,
    buffer: Unique<ScreenBuffer>,
}

pub static VGA_WRITER: Mutex<VGA> = Mutex::new(
    VGA
    {
        column_position: 0,
        color_code: ColorCode::new(Color::LightGreen,
                                 Color::Black),
        buffer: unsafe { Unique::new(0xb8000 as *mut _) },
    }
);

pub fn print(args: fmt::Arguments) {
    use core::fmt::Write;
    VGA_WRITER.lock().write_fmt(args).unwrap();
}

impl VGA {

    fn buffer(&mut self) -> &mut ScreenBuffer {
        unsafe { self.buffer.get_mut() }
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

                self.buffer().chars[row][col].write(ScreenChar {
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

        let col = self.column_position - 1;
        let blank = GREEN_BLANK;
        self.column_position -= 1;
    }

    pub fn new_line(&mut self) {
        for row in 1..BUFFER_HEIGHT {
            for col in 0..BUFFER_WIDTH {
                let buffer = self.buffer();
                let character = buffer.chars[row][col].read();
                buffer.chars[row - 1][col].write(character);
            }
        }

        self.clear_row(BUFFER_HEIGHT - 1);
        self.column_position = 0;
    }


    fn clear_row(&mut self, row: usize) {
        let blank = ScreenChar {
            ascii_character: b' ',
            color_code: self.color_code,
        };
        for col in 0..BUFFER_WIDTH {
            self.buffer().chars[row][col].write(blank);
        }
    }

    #[allow(dead_code)]
    pub fn clear(&mut self) {
        for _ in 0..BUFFER_HEIGHT {
            self.new_line();
        }
    }
}

impl fmt::Write for VGA {
    fn write_str(&mut self, s: &str) -> ::core::fmt::Result {
        for byte in s.bytes() {
            self.write_byte(byte)
        }
        Ok(())
    }
}