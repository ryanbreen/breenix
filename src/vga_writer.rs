use core::ptr::Unique;
use spin::Mutex;

use constants::vga::{BUFFER_WIDTH, BUFFER_HEIGHT};

use buffers::Buffer;

#[repr(u8)]
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

#[derive(Clone, Copy)]
pub struct ColorCode(u8);

impl ColorCode {
    pub const fn new(foreground: Color, background: Color) -> ColorCode {
        ColorCode((background as u8) << 4 | (foreground as u8))
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ScreenChar {
    pub ascii_character: u8,
    pub color_code: ColorCode,
}

pub struct VgaWriter {
    buffer: Unique<Buffer>,
}

impl VgaWriter {
    pub fn write_buffer(&mut self, buffer: &Buffer) {
        for row in 0..BUFFER_HEIGHT {
            for col in 0..BUFFER_WIDTH {
                self.write_char(row, col, buffer.chars[row][col]);
            }
        }
    }

    pub fn write_char(&mut self, row: usize, col: usize, sc: ScreenChar) {
        unsafe {
            self.buffer.get_mut().chars[row][col] = sc;
        }
        update_cursor(row as u8, (col + 1) as u8);
    }
}

#[allow(exceeding_bitshifts)]
pub fn update_cursor(row: u8, col: u8) {
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

pub static mut VGA_WRITER: Mutex<VgaWriter> =
    Mutex::new(VgaWriter { buffer: unsafe { Unique::new(0xb8000 as *mut _) } });
