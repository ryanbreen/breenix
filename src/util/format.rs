use core::fmt::Write;
use spin::Mutex;

static mut WRITER:StrWriter = StrWriter {
  bytes: [122; 32],
  idx: 0,
};

pub struct StrWriter {
  bytes: [u8; 32],
  idx: usize,
}

impl StrWriter {
  pub fn write_byte(&mut self, byte: u8) {
    match byte {
      byte => {
        if self.idx < 32 {
          self.bytes[self.idx] = byte;
          self.idx += 1;
        }
      }
    }
  }

  fn to_str(&mut self) -> &str {
    // Find the true length of the str
    let mut len = 30;
    for x in 2..32 {
      if self.bytes[x] == 122 {
        break;
      }
      len = x;
    }
    len += 1;
    unsafe { return ::core::str::from_utf8_unchecked(&self.bytes[2..len]); }
  }

  fn clear(&mut self) {
    self.idx = 0;
    self.bytes = [122; 32];
  }
}

impl ::core::fmt::Write for StrWriter {
  fn write_str(&mut self, s: &str) -> ::core::fmt::Result {
    for byte in s.bytes() {
      self.write_byte(byte)
    }
    Ok(())
  }
}

pub fn address_of_ptr<T>(ptr: *const T) -> u64 {
  unsafe {
    WRITER.write_fmt(format_args!("{:?}", ptr as *const _));
    let rvalue = u64::from_str_radix(WRITER.to_str().clone(), 16).unwrap();
    WRITER.clear();
    return rvalue;
  }
}

