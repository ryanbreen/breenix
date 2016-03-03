const IDT_SIZE: usize = 256;

#[repr(C)]
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

#[repr(C)]
#[derive(Clone, Copy)]
struct IDTable {
  limit: u16,
  base: *const [IDTEntry;IDT_SIZE]
}

static mut counter:u64 = 0;
static mut test_success:bool = false;
static mut idt_init:bool = false;

extern {
  static _asm_irq_handler_array: [u64;IDT_SIZE as usize];
}

pub fn get_irq_handler(num: u16) -> u64 {
  _asm_irq_handler_array[num as usize]
}

#[no_mangle]
#[inline(never)]
pub unsafe extern "C" fn idt_test_handler() {
  println!("Test handler called");
  test_success = true;
}

#[no_mangle]
#[inline(never)]
pub unsafe extern "C" fn idt_default_handler() {
  println!("Default handler");
  counter += 1;
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

  descriptors[num].clbk_low  = ((clbk as u64) & 0xFFFF) as u16;
  descriptors[num].clbk_mid  = (((clbk as u64) >> 16) & 0xFFFF) as u16;
  descriptors[num].clbk_high = (((clbk as u64) >> 32) & 0xFFFFFFFF) as u32;
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

    // FIXME: this shouldn't be necessary (see above)
    idt_table.limit = (IDT_SIZE as u16) * 8;
    idt_table.base = &descriptors as *const [IDTEntry;IDT_SIZE];

    let clbk_addr = &idt_default_handler as *const _ as u64;
    for i in 0..IDT_SIZE as u16 {
      //let clbk_addr = get_irq_handler(i);
      load_descriptor(i as usize, clbk_addr, 0x8E, 0x08);
    }

    let fn_ptr = &idt_test_handler as *const _ as u64;
    println!("{}", test_success);
    (*(fn_ptr as *const fn()))();
    println!("{}", test_success);
    //load_descriptor(0x2f, fn_ptr, 0x8E, 0x08);
    //println!("Initted test handler {:x}", fn_ptr);

    let idt_table_address = idt_table.base as u64;
    let entry_at_offset = idt_table_address + (0x2F*0x10);
    println!("idt starts at {:x}, entry at {:x}, delta {:x}", idt_table_address, entry_at_offset, entry_at_offset - idt_table_address);
    
    let idt_entry = *(entry_at_offset as *const IDTEntry);
    println!("{:?}", idt_entry);
    println!("{:x}", (idt_entry.clbk_high as u64) << 32 | (idt_entry.clbk_mid as u64) << 16 | idt_entry.clbk_low as u64);

    let address_of_fn = (idt_entry.clbk_high as u64) << 32 | (idt_entry.clbk_mid as u64) << 16 | idt_entry.clbk_low as u64;
    (*(address_of_fn as *const fn()))();
    //println!("{:?}", my_fun);
    //my_fun();

    asm!("lidt ($0)" :: "r" (&idt_table as *const _ as u64));
    //asm!("sti");
    //asm!("int $$0x2f" :::: "volatile");
    //asm!("int $$0x12" :::: "volatile");
    
    //idt_test_handler();
    println!("{}", test_success);

  }
}
