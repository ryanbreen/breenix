mod idt;

use buffers::print_error;

use io::ChainedPics;

use spin::Mutex;

extern "C" fn page_fault_handler_wrapper() -> ! {

  let mut ic:InterruptContext = InterruptContext::empty();

  unsafe {
    asm!("" : "={rax}"(ic.rax));
    asm!("push %rax");
    asm!("" : "={rcx}"(ic.rcx));
    asm!("push %rcx");
    asm!("" : "={rdx}"(ic.rdx));
    asm!("push %rdx");
    asm!("" : "={r8}"(ic.r8));
    asm!("push %r8");
    asm!("" : "={r9}"(ic.r9));
    asm!("push %r9");
    asm!("" : "={r10}"(ic.r10));
    asm!("push %r10");
    asm!("" : "={r11}"(ic.r11));
    asm!("push %r11");
    asm!("" : "={rdi}"(ic.rdi));
    asm!("push %rdi");
    asm!("" : "={rdi}"(ic.rsi));
    asm!("push %rsi");

    ic.int_id = 14;
    page_fault_handler(&ic);

    // Now pop everything back off the stack and to the registers.
    asm!("pop rsi");
    asm!("pop rdi");
    asm!("pop r11");
    asm!("pop r10");
    asm!("pop r9");
    asm!("pop r8");
    asm!("pop rdx");
    asm!("pop rcx");
    asm!("pop rax");
  }

}

extern "C" fn page_fault_handler(ctx: &InterruptContext) -> ! {

    unsafe { print_error(format_args!("EXCEPTION: PAGE FAULT {:?}", ctx)) };

    loop {}
}

lazy_static! {
    static ref IDT: idt::Idt = {
        let mut idt = idt::Idt::new();

        idt.set_handler(14, page_fault_handler_wrapper);

        idt
    };
}

pub fn init() {
    IDT.load();
}

/// Interface to our PIC (programmable interrupt controller) chips.  We
/// want to map hardware interrupts to 0x20 (for PIC1) or 0x28 (for PIC2).
pub static PICS: Mutex<ChainedPics> = Mutex::new(unsafe { ChainedPics::new(0x20, 0x28) });

/// Various data available on our stack when handling an interrupt.
#[repr(C, packed)]
#[derive(Debug)]
struct InterruptContext {
    rsi: u64,
    rdi: u64,
    r11: u64,
    r10: u64,
    r9: u64,
    r8: u64,
    rdx: u64,
    rcx: u64,
    rax: u64,
    int_id: u32,
    _pad_1: u32,
    error_code: u32,
    _pad_2: u32,
}

impl InterruptContext {
  fn empty() -> InterruptContext {
    InterruptContext {
      rsi: 0,
      rdi: 0,
      r11: 0,
      r10: 0,
      r9: 0,
      r8: 0,
      rdx: 0,
      rcx: 0,
      rax: 0,
      int_id: 0,
      _pad_1: 0,
      error_code: 0,
      _pad_2: 0,
    }
  }
}
