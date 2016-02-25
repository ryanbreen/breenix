use core::fmt::Write;
use spin::Mutex;

static mut WRITER:StrWriter = StrWriter {
  bytes: [0; 8],
  idx: 0,
};

pub struct StrWriter {
  bytes: [u8; 8],
  idx: usize,
}

impl StrWriter {
  pub fn write_byte(&mut self, byte: u8) {
    match byte {
      byte => {
        if self.idx < 8 {
          self.bytes[self.idx] = byte;
          self.idx += 1;
        }
      }
    }
  }

  fn to_str(&mut self) -> &str {
    self.idx = 0;
    unsafe { return ::core::str::from_utf8_unchecked(&self.bytes); }
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

pub fn address_str_of_ptr<T>(ptr: *const T) -> &'static str {
  unsafe {
    WRITER.write_fmt(format_args!("{:?}", &ptr as *const _));
    return WRITER.to_str().clone();
  }
}

pub fn address_of_ptr<T>(ptr: *const T) -> u32 {
  let address = address_str_of_ptr(ptr);
  return u32::from_str_radix(&(address[2..]), 16).unwrap();
}
