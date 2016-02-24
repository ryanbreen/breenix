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

pub fn address_of_array(ptr: &[u8]) -> &str {
  unsafe {
    WRITER.write_fmt(format_args!("{:?}", &ptr as *const _));
    return WRITER.to_str().clone();
  }
}

#[repr(C)]
struct IDT {
  test_timeout: u16,
  idt_location: u32,
  idtr_location: u32,
  test_success: u8,
  setup: bool,
  data: [u8;0x800]
}

impl IDT {
  fn new() -> IDT
  {
      let mut idt = IDT {
          test_timeout: 0x1000,
          test_success: 0,
          idt_location: 0,
          idtr_location: 0,
          data: [0; 0x800],
          setup: false
      };
      idt.initialize();
      return idt;
  }

  // Cribbed from https://github.com/levex/osdev/blob/master/arch/idt.c#L28
  fn initialize(&mut self) {

    let address = address_of_array(&self.data);
    println!("IDT location as a str: {}", address);
    self.idt_location = u32::from_str_radix(&(address[2..]), 16).unwrap();
    println!("IDT: Location: {:x}", self.idt_location);
    println!("IDT: Location: {}", self.idt_location);
    println!("IDT: IDTR Location: {:x}", self.idtr_location);

    self.setup = true;
  }

  fn idt_register_interrupt(&mut self, idx: u8, callback: u32) {
    if !self.setup {
      panic!("Invalid IDT!");
    }
    /*
    self.idt_location
    *(uint16_t*)(idt_location + 8*i + 0) = (uint16_t)(callback & 0x0000ffff);
    *(uint16_t*)(idt_location + 8*i + 2) = (uint16_t)0x8;
    *(uint8_t*) (idt_location + 8*i + 4) = 0x00;
    *(uint8_t*) (idt_location + 8*i + 5) = 0x8e;//0 | IDT_32BIT_INTERRUPT_GATE | IDT_PRESENT;
    *(uint16_t*)(idt_location + 8*i + 6) = (uint16_t)((callback & 0xffff0000) >> 16);
    if(test_success) mprint("Registered INT#%d\n", i);
    return;
    */
  }
}

pub fn test() {
  let _ = IDT::new();
}