
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
    //self.idt_location = (&self.data as *mut u8) as u32;
    self.idt_location = 0x100;
    println!("IDT: Location: {:x}", self.idt_location);
    self.idtr_location = 0x10F0;
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