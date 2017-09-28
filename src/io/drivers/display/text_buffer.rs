use spin::Mutex;
use io::drivers::display::vga::{VGA, Color, ColorCode};

use constants::vga::{BUFFER_WIDTH, BUFFER_HEIGHT};

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

pub fn print(s: &str) {
    use core::fmt::Write;
    PRINT_BUFFER.lock().write_str(s).unwrap();
}

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

pub static mut ACTIVE_BUFFER: &'static Mutex<TextBuffer> = &PRINT_BUFFER;
static mut INACTIVE_BUFFERS: [&'static Mutex<TextBuffer>; 2] = [&DEBUG_BUFFER, &KEYBOARD_BUFFER];

pub fn toggle() {
    unsafe {
        ACTIVE_BUFFER.lock().deactivate();

        let new_active = INACTIVE_BUFFERS[0];
        INACTIVE_BUFFERS[0] = INACTIVE_BUFFERS[1];
        INACTIVE_BUFFERS[1] = ACTIVE_BUFFER;
        ACTIVE_BUFFER = new_active;
        ACTIVE_BUFFER.lock().activate();
        ACTIVE_BUFFER.lock().sync();
    }
}
