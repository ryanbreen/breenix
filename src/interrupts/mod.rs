mod idt;

use buffers::print_error;

use io::ChainedPics;

use spin::Mutex;

extern "C" fn page_fault_handler() -> ! {
    unsafe { print_error(format_args!("EXCEPTION: PAGE FAULT")) };

    loop {}
}

lazy_static! {
    static ref IDT: idt::Idt = {
        let mut idt = idt::Idt::new();

        idt.set_handler(14, page_fault_handler);

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
