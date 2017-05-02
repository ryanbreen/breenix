use core::ptr::Unique;
use spin::Mutex;

use constants::vga::{BUFFER_WIDTH, BUFFER_HEIGHT};

use writers::Buffer;

/*
pub struct VgaWriter {
    pub buffer: Unique<Buffer>,
}

impl VgaWriter {

    fn buffer(&mut self) -> &mut Buffer {
        unsafe { self.buffer.get_mut() }
    }

    pub fn write_buffer(&mut self, buffer: &Buffer) {
        for row in 0..BUFFER_HEIGHT {
            for col in 0..BUFFER_WIDTH {
                self.write_char(row, col, buffer.chars[row][col]);
            }
        }
    }

    pub fn write_char(&mut self, row: usize, col: usize, sc: ScreenChar) {
        unsafe {
            //self.buffer().chars[row][col].write(sc);
            self.buffer().chars[row][col].write(ScreenChar {
                                                        ascii_character: b'A',
                                                        color_code: ColorCode::new(Color::LightGreen, Color::Black),
                                                    });
        }
        update_cursor(row as u8, (col + 1) as u8);
    }
}

#[allow(exceeding_bitshifts)]
pub fn update_cursor(row: u8, col: u8) {
    /*
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
    */
}

pub static mut VGA_WRITER: Mutex<VgaWriter> =
    Mutex::new(VgaWriter { buffer: unsafe { Unique::new(0xb8000 as *mut _) } });
*/