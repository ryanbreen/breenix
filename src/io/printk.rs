use alloc::boxed::Box;
use collections::Vec;
use core::fmt;
use core::mem;
use collections::string::String;

use io::serial;

use spin::Mutex;

pub struct PrintkBuffer {
  buffer: Box<Vec<String>>,
  partial: String,
}

impl ::core::fmt::Write for PrintkBuffer {
  fn write_str(&mut self, ss: &str) -> ::core::fmt::Result {

      let mut s = String::from(ss);
      let endline = s.find('\n').unwrap_or(255);

      match endline {
        255 => self.partial += ss,
        _ => {
          let remainder = s.split_off(endline);
          let line = self.partial.clone() + &s;
          serial::write(&line);
          self.buffer.push(line);
          self.partial = remainder;
        }
      };

      // self.buffer.push(string);

      // serial::write(s);

      Ok(())
  }
}

static mut PRINTK_BUFFER: Option<Mutex<PrintkBuffer>> = None;

pub fn init() {
  unsafe {
    // Create a new BUFFER
    PRINTK_BUFFER = Some(Mutex::new(PrintkBuffer {
      buffer: Box::new(vec!()),
      partial: String::new(),
    }));
  }
}

pub fn print(args: fmt::Arguments) {
    unsafe {
      match PRINTK_BUFFER {
        Some(ref mut pk) => {
          use core::fmt::Write;
          let mut pb = pk.lock();
          (*pb).write_fmt(args).unwrap()
        },
        None => {},
      }
    }
}
