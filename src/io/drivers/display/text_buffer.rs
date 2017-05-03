use core::fmt;

use spin::Mutex;

use io::drivers::display::vga;
use io::drivers::display::vga::{VGA, Color, ScreenChar, ColorCode};

use io::serial;

use constants::vga::{BUFFER_WIDTH, BUFFER_HEIGHT, GREEN_BLANK};

pub struct TextBuffer {
    chars: [[u8; BUFFER_WIDTH]; BUFFER_HEIGHT],
    column_position: usize,
    color_code: ColorCode,
    blank_char: ScreenChar,
    active: bool,
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
            unsafe { VGA.lock().sync_buffer(&self); }
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
                let cc = self.color_code;
                self.chars[row][col] = byte;
                self.column_position += 1;
            }
        }

        self.sync();
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

        serial::write(s);

        Ok(())
    }
}

pub static PRINT_BUFFER: Mutex<TextBuffer> = Mutex::new(TextBuffer {
    column_position: 0,
    color_code: ColorCode::new(Color::LightGreen, Color::Black),
    blank_char: GREEN_BLANK,
    chars: [[b' '; BUFFER_WIDTH]; BUFFER_HEIGHT],
    active: true,
});

pub fn print(args: fmt::Arguments) {
    use core::fmt::Write;
    PRINT_BUFFER.lock().write_fmt(args).unwrap();
}

/*
pub static KEYBOARD_BUFFER: Mutex<Buffer> = Mutex::new(Buffer {
    column_position: 0,
    color_code: ColorCode::new(Color::LightRed, Color::Black),
    blank_char: RED_BLANK,
    chars: [[RED_BLANK; BUFFER_WIDTH]; BUFFER_HEIGHT],
    active: false,
});

pub static DEBUG_BUFFER: Mutex<Buffer> = Mutex::new(Buffer {
    column_position: 0,
    color_code: ColorCode::new(Color::LightGray, Color::Black),
    blank_char: GRAY_BLANK,
    chars: [[GRAY_BLANK; BUFFER_WIDTH]; BUFFER_HEIGHT],
    active: false,
});

pub static mut ACTIVE_BUFFER: &'static Mutex<Buffer> = &PRINT_BUFFER;
static mut INACTIVE_BUFFERS: [&'static Mutex<Buffer>; 2] = [&DEBUG_BUFFER, &KEYBOARD_BUFFER];

pub fn toggle() {
    unsafe {
        ACTIVE_BUFFER.lock().deactivate();

        let new_active = INACTIVE_BUFFERS[0];
        INACTIVE_BUFFERS[0] = INACTIVE_BUFFERS[1];
        INACTIVE_BUFFERS[1] = ACTIVE_BUFFER;
        ACTIVE_BUFFER = new_active;
        ACTIVE_BUFFER.lock().activate();

        vga_writer::update_cursor(BUFFER_HEIGHT as u8 - 1,
                                  ACTIVE_BUFFER.lock().column_position as u8);
    }
}
*/

/// Our printer of last resort.  This is guaranteed to write without trying to grab a lock that
/// may be held by someone else.
#[allow(unused_must_use)]
pub unsafe fn print_error(fmt: fmt::Arguments) {
    use core::fmt::Write;
    use core::ptr::Unique;
/*
    let mut error_buffer = Buffer {
        column_position: 0,
        color_code: ColorCode::new(Color::Red, Color::Black),
        blank_char: RED_BLANK,
        chars: [[RED_BLANK; BUFFER_WIDTH]; BUFFER_HEIGHT],
        active: true,
    };

    vga_writer::VgaWriter {
        buffer: Unique::new(0xb8000 as *mut _),
    };
    error_buffer.new_line();
    error_buffer.write_fmt(fmt);
    */
}
