
#[repr(C, packed)]
#[derive(Copy, Clone, Debug)]
struct IDTEntry {
  clbk_low: u16,
  selector: u16,
  zero: u8,
  flags: u8,
  clbk_mid: u16,
  clbk_high: u32,
  zero2: u32
}

#[repr(C, packed)]
struct IDTable {
  limit: u16,
  base: *const [IDTEntry;IDT_SIZE]
}

const IDT_SIZE: usize = 256;

static mut test_success:bool = false;
static mut idt_init:bool = false;

#[no_mangle]
pub extern "C" fn idt_test_handler() {
  unsafe {
    println!("Test handler called");
    test_success = true;
  }
}

#[no_mangle]
pub extern "C" fn idt_default_handler() {
  println!("Default handler");
}

// The table itself, an array of 256 entries.
// All the entries are statically initialized so that all interrupts are by
// default handled by a function that do nothing.
// Specialized handlers will come later
static mut descriptors: [IDTEntry;IDT_SIZE] = [IDTEntry {
    clbk_low:  0,
    clbk_mid:  0,
    clbk_high: 0,
    selector: 0x08,
    flags: 0x8E,
    zero: 0,
    zero2: 0
};IDT_SIZE];

static mut idt_table: IDTable = IDTable {
  limit: 0, 
  base: 0 as *const [IDTEntry;IDT_SIZE]
};

pub unsafe fn load_descriptor(num: usize, clbk: u64, flags: u8, selector: u16) {
  if num >= IDT_SIZE {
    println!("Invalid interrupt {}", num);
    return;
  }

  descriptors[num].clbk_low  = (clbk & 0xFFFF) as u16;
  descriptors[num].clbk_mid  = ((clbk >> 16) & 0xFFFF) as u16;
  descriptors[num].clbk_high = ((clbk >> 32) & 0xFFFFFFFF) as u32;
  descriptors[num].selector = selector;
  descriptors[num].flags = flags;
}

// Cribbed from https://github.com/levex/osdev/blob/master/arch/idt.c#L28 and
// https://github.com/LeoTestard/Quasar/blob/master/arch/x86_64/idt.rs
pub fn setup() {
  unsafe {
    if idt_init {
      // IDT already initialized
      return;
    }

    idt_init = false;

    // FIXME: this souldn't be necessary (see above)
    idt_table.limit = (IDT_SIZE as u16) * 8;
    idt_table.base = &descriptors as *const [IDTEntry;256];

    // FIXME: this shouldn't be necessary (see above)
    let mut i = 0;
    let clbk_addr = &idt_default_handler as *const _ as u64;
    while i < IDT_SIZE {
      load_descriptor(i, clbk_addr, 0x8E, 0x08);
      i += 1
    }

    let fn_ptr = &idt_test_handler as *const _ as u64;
    load_descriptor(0x2f, fn_ptr, 0x8E, 0x08);

    let idt_table_address = (&idt_table as * const _ as u64);
    let entry_at_offset = idt_table_address + (0x2F*0x80);
    println!("idt starts at {}, entry at {}, delta {}", idt_table_address, entry_at_offset, entry_at_offset - idt_table_address);
    //println!("{:?}", *(entry_at_offset as *const IDTEntry));

    println!("Initted test handler {}", fn_ptr);

    asm!("lidt ($0)" :: "r" (idt_table_address));
    //asm!("sti");
    asm!("int $$0x2f" :::: "volatile");
  }
}


