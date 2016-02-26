use util::format;

#[repr(C)]
struct IDT {
  test_timeout: u16,
  idt_location: u32,
  idtr_location: u32,
  setup: bool,
  data: [u8;0x800],
  idtr: [u16;3],
}

static mut test_success:bool = false;
#[no_mangle]
pub extern "C" fn idt_test_handler() {
  unsafe {
    test_success = true;
  }
}

#[no_mangle]
pub extern "C" fn idt_default_handler() {
  println!("Default handler");
}

impl IDT {
  fn new() -> IDT
  {
      let mut idt = IDT {
          test_timeout: 0x1000,
          setup: false,
          idt_location: 0,
          idtr_location: 0,
          data: [0; 0x800],
          idtr: [0; 3]
      };
      idt.initialize();
      return idt;
  }

  // Cribbed from https://github.com/levex/osdev/blob/master/arch/idt.c#L28
  fn initialize(&mut self) {

    let address = format::address_of_ptr(&self.data);
    println!("IDT location as a str: {}", address);
    self.idt_location = address;
    println!("IDT: Location: 0x{:x}", self.idt_location);

    self.setup = true;

    for i in 0..255 {
      //self.idt_register_interrupt(i, format::address_of_ptr(&idt_default_handler as *const _));
    }

    // Try to set the test id
    println!("IDT test handler location: {:?}", &idt_test_handler as *const _);
    let fn_ptr = format::address_of_ptr(&idt_test_handler as *const _);

    self.idt_register_interrupt(0x2f, fn_ptr);
    println!("Test fn lives at 0x{:x}", fn_ptr);

    // Initialize idtr
    self.idtr[0] = 0x800-1;
    self.idtr[1] = ((self.idt_location & 0xffff0000) >> 16) as u16;
    self.idtr[2] = (self.idt_location & 0x0000ffff) as u16;

    self.idtr_location = format::address_of_ptr(&self.data);
    println!("IDT: IDTR Location: 0x{:x}", self.idtr_location);

    unsafe {
      asm!("lidt %idtr" :: "{idtr}"(self.idtr_location) :: "volatile");
      asm!("int $$0x2f" :::: "volatile");
    }
  }

  fn idt_register_interrupt(&mut self, idx: u8, callback: u32) {
    if !self.setup {
      panic!("Invalid IDT!");
    }

    let i:usize = idx as usize * 8; // Each IDT entry is 12 bytes
    //(uint16_t*)(idt_location + 8*i + 0) = (uint16_t)(callback & 0x0000ffff);
    self.data[i] =   ((callback & 0x0000ff00) >> 8) as u8;
    self.data[i+1] = (callback & 0x000000ff) as u8;
    //*(uint16_t*)(idt_location + 8*i + 2) = (uint16_t)0x8;
    self.data[i+2] = 0x8 as u8;
    //*(uint8_t*) (idt_location + 8*i + 4) = 0x00;
    self.data[i+4] = 0x00 as u8;
    //*(uint8_t*) (idt_location + 8*i + 5) = 0x8e;//0 | IDT_32BIT_INTERRUPT_GATE | IDT_PRESENT;
    self.data[i+5] = 0x8e as u8; //0 | IDT_32BIT_INTERRUPT_GATE | IDT_PRESENT;
    //*(uint16_t*)(idt_location + 8*i + 6) = (uint16_t)((callback & 0xffff0000) >> 16);
    self.data[i+6] = ((callback & 0xffff0000) >> 24) as u8;
    self.data[i+7] = ((callback & 0xffff0000) >> 16) as u8;
    println!("{:x} {:x} {:x} {:x}", self.data[i+6], self.data[i+7], self.data[i], self.data[i+1]);
    unsafe {
      if test_success {
        println!("Registered INT#{}", idx);
      }
    }
  }
}

pub fn test() {
  let _ = IDT::new();
}