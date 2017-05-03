use constants::vga::{BUFFER_WIDTH, BUFFER_HEIGHT};
use constants::vga::{GREEN_BLANK, RED_BLANK};

use io::drivers::display::text_buffer::TextBuffer;

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
    frame: Unique<ScreenBuffer>,
}

pub static VGA: Mutex<VGA> = Mutex::new(VGA { frame: unsafe { Unique::new(0xb8000 as *mut _) } });

impl VGA {

    fn frame(&mut self) -> &mut ScreenBuffer {
        unsafe { self.frame.get_mut() }
    }

    pub fn sync_buffer(&mut self, buffer: &TextBuffer) {
        let frame = self.frame();
        for row in 0..BUFFER_HEIGHT {
            for col in 0..BUFFER_WIDTH {
                let character = ScreenChar {
                    ascii_character: buffer.chars()[row][col],
                    color_code: buffer.color_code(),
                };
                frame.chars[row][col].write(character);
            }
        }
    }
}
