use constants::vga::{BUFFER_WIDTH, BUFFER_HEIGHT};

use io::drivers::display::text_buffer::TextBuffer;

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

pub static VGA: Mutex<VGA> = Mutex::new(VGA { frame: unsafe { Unique::new_unchecked(0xb8000 as *mut _) } });

impl VGA {

    fn frame(&mut self) -> &mut ScreenBuffer {
        unsafe { self.frame.as_mut() }
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

    #[allow(exceeding_bitshifts)]
    pub fn update_cursor(&self, row: usize, col: usize) {
        let position: u16 = (row as u16 * (BUFFER_WIDTH as u16)) + col as u16;
        use io::Port;

        unsafe {
            let mut cursor_control_port: Port<u8> = Port::new(0x3D4);
            let mut cursor_value_port: Port<u8> = Port::new(0x3D5);

            // cursor HIGH port to vga INDEX register
            cursor_control_port.write(0x0E);
            cursor_value_port.write(((position >> 8) & 0xFF) as u8);
            // cursor LOW port to vga INDEX register
            cursor_control_port.write(0x0F);
            cursor_value_port.write((position & 0xFF) as u8);
        }
    }
}
