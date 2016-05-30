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
