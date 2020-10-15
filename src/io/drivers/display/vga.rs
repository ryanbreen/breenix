use crate::constants::vga::{Color, BUFFER_HEIGHT, BUFFER_WIDTH};

use crate::io::drivers::display::text_buffer::TextBuffer;

use core::ptr::Unique;

use spin::Mutex;

use volatile::Volatile;

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

pub static VGA: Mutex<VGA> = Mutex::new(VGA {
    frame: unsafe { Unique::new_unchecked(0xb8000 as *mut _) },
});

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

    #[allow(arithmetic_overflow)]
    pub fn update_cursor(&self, row: usize, col: usize) {
        let position: u16 = (row as u16 * (BUFFER_WIDTH as u16)) + col as u16;
        use crate::io::Port;

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
