
struct IDT {
  test_timeout: u16,
  idt_size: u16,
  idt_location: u32,
  idtr_location: u32,
  test_success: u8,
}

impl IDT {
  pub fn new() -> IDT
  {
      let mut idt = IDT {
          test_timeout: 0x1000,
          test_success: 0,
          idt_location: 0,
          idtr_location: 0,
          idt_size: 0x800
      };
      idt.initialize();
      return idt;
  }

  fn initialize(&mut self) {
    self.idt_location = 0x2000;
    println!("IDTR: Location: {:x}", self.idt_location);
  }
}

pub fn test() {
  let _ = IDT::new();
}