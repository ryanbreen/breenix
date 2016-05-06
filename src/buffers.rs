use spin::Mutex;

use vga_writer;
use vga_writer::{ScreenChar, ColorCode, Color};

use constants::vga::{GREEN_BLANK,GRAY_BLANK,RED_BLANK,BUFFER_WIDTH,BUFFER_HEIGHT};

use io::serial;
use io::timer;

pub struct Buffer {
  pub chars: [[ScreenChar; BUFFER_WIDTH]; BUFFER_HEIGHT],
  active: bool,
  column_position: usize,
  blank_char: ScreenChar,
  color_code: ColorCode,
}

impl Buffer {
  pub fn activate(&mut self) {
    self.active = true;
    unsafe { vga_writer::VGA_WRITER.lock().write_buffer(&self); }
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
    if self.column_position == 0 {
      return;
    }

    let col = self.column_position-1;
    let blank = self.blank_char;
    self.write_to_buffers(BUFFER_HEIGHT-1, col, blank);
    self.column_position -= 1;

    if self.active {
      vga_writer::update_cursor(BUFFER_HEIGHT as u8 -1, self.column_position as u8);
    }
  }

  fn write_to_buffers(&mut self, row: usize, col: usize, sc:ScreenChar) {
    if self.active {
      unsafe { vga_writer::VGA_WRITER.lock().write_char(row, col, sc); }
    }

    self.chars[row][col] = sc;
  }

  pub fn new_line(&mut self) {
    for row in 0..(BUFFER_HEIGHT-1) {
      for col in 0..BUFFER_WIDTH {
        let sc = self.chars[row + 1][col];
        self.write_to_buffers(row, col, sc);  
      }
    }

    let blank = self.blank_char;
    for col in 0..BUFFER_WIDTH {
      self.write_to_buffers(BUFFER_HEIGHT-1, col, blank);
    }
    self.column_position = 0;

    if self.active {
      vga_writer::update_cursor(BUFFER_HEIGHT as u8 -1, 0);
    }
  }

  #[allow(dead_code)]
  pub fn clear(&mut self) {
    for _ in 0..BUFFER_HEIGHT {
      self.new_line();
    }
  }
}

impl ::core::fmt::Write for Buffer {
  fn write_str(&mut self, s: &str) -> ::core::fmt::Result {
    for byte in s.bytes() {
      self.write_byte(byte)
    }

    serial::write(s);

    Ok(())
  }
}

pub static PRINT_BUFFER: Mutex<Buffer> = Mutex::new(Buffer {
  column_position: 0,
  color_code: ColorCode::new(Color::LightGreen, Color::Black),
  blank_char: GREEN_BLANK,
  chars: [[GREEN_BLANK; BUFFER_WIDTH]; BUFFER_HEIGHT],
  active: true,
});

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

#[allow(unused_must_use)]
pub fn debug() {
  use core::fmt::Write;
  use x86::msr::{IA32_EFER,TSC,MSR_MCG_RFLAGS};
  use x86::msr::rdmsr;
  use x86::time::rdtsc;

  use memory;

  let mut buffer = DEBUG_BUFFER.lock();
  unsafe {
    let time = timer::time_since_start();
    buffer.write_fmt(format_args!("-------------------------------\n"));
    buffer.write_fmt(format_args!("Time: {}.{}\n", time.secs, time.nanos));
    buffer.write_fmt(format_args!("rdtsc: 0x{:x}\n", rdtsc()));
    buffer.write_fmt(format_args!("msr IA32_EFER: 0x{:x}\n", rdmsr(IA32_EFER)));
    buffer.write_fmt(format_args!("msr TSC: 0x{:x}\n", rdmsr(TSC)));
    buffer.write_fmt(format_args!("msr MSR_MCG_RFLAGS: 0x{:x}\n", rdmsr(MSR_MCG_RFLAGS)));
    buffer.write_fmt(format_args!("interrupt count: 0x20={}, 0x21={}, 0x80={}\n",
      ::state().interrupt_count[0x20], ::state().interrupt_count[0x21], ::state().interrupt_count[0x80]));
    buffer.write_fmt(format_args!("allocated frame count: 0={}\n",
      memory::frame_allocator().allocated_frame_count()));
    buffer.write_fmt(format_args!("{:?}\n", memory::slab_allocator::zone_allocator()));
  }
}

pub static mut ACTIVE_BUFFER:&'static Mutex<Buffer> = &PRINT_BUFFER;
static mut INACTIVE_BUFFERS:[&'static Mutex<Buffer>;2] = [&DEBUG_BUFFER, &KEYBOARD_BUFFER];

pub fn toggle() {
  unsafe {
    ACTIVE_BUFFER.lock().deactivate();

    let new_active = INACTIVE_BUFFERS[0];
    INACTIVE_BUFFERS[0] = INACTIVE_BUFFERS[1];
    INACTIVE_BUFFERS[1] = ACTIVE_BUFFER;
    ACTIVE_BUFFER = new_active;
    ACTIVE_BUFFER.lock().activate();

    vga_writer::update_cursor(BUFFER_HEIGHT as u8 -1, ACTIVE_BUFFER.lock().column_position as u8);
  }
}
