use crate::gdt;

use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame, PageFaultErrorCode};
use pic8259::ChainedPics;
use spin;

pub const PIC_1_OFFSET: u8 = 32;
pub const PIC_2_OFFSET: u8 = PIC_1_OFFSET + 8;

pub static PICS: spin::Mutex<ChainedPics> =
    spin::Mutex::new(unsafe { ChainedPics::new(PIC_1_OFFSET, PIC_2_OFFSET) });

#[derive(Debug, Clone, Copy)]
#[repr(u8)]
pub enum InterruptIndex {
    Timer = PIC_1_OFFSET,
    Keyboard,
}

impl InterruptIndex {
    fn as_u8(self) -> u8 {
        self as u8
    }

    #[allow(dead_code)]
    fn as_usize(self) -> usize {
        usize::from(self.as_u8())
    }
}

static mut IDT: InterruptDescriptorTable = InterruptDescriptorTable::new();

pub fn init() {
    // Initialize GDT first
    gdt::init();
    // Then initialize IDT
    init_idt();
}

pub fn init_idt() {
    unsafe {
        // CPU exception handlers
        IDT.divide_error.set_handler_fn(divide_by_zero_handler);
        IDT.breakpoint.set_handler_fn(breakpoint_handler);
        IDT.invalid_opcode.set_handler_fn(invalid_opcode_handler);
        IDT.double_fault.set_handler_fn(double_fault_handler)
            .set_stack_index(gdt::DOUBLE_FAULT_IST_INDEX);
        IDT.page_fault.set_handler_fn(page_fault_handler);
        
        // Hardware interrupt handlers
        IDT[InterruptIndex::Timer.as_u8()].set_handler_fn(timer_interrupt_handler);
        IDT[InterruptIndex::Keyboard.as_u8()].set_handler_fn(keyboard_interrupt_handler);
        
        // Set up a generic handler for all unhandled interrupts
        for i in 32..=255 {
            if i != InterruptIndex::Timer.as_u8() && i != InterruptIndex::Keyboard.as_u8() {
                IDT[i].set_handler_fn(generic_handler);
            }
        }
        
        IDT.load();
    }
    log::info!("IDT loaded successfully");
}

pub fn init_pic() {
    unsafe {
        // Initialize the PIC
        PICS.lock().initialize();
        
        // Unmask keyboard interrupt (IRQ1) and timer interrupt (IRQ0)
        use x86_64::instructions::port::Port;
        let mut port: Port<u8> = Port::new(0x21); // PIC1 data port
        let mask = port.read() & !0b11; // Clear bit 0 (timer) and bit 1 (keyboard)
        port.write(mask);
    }
}

extern "x86-interrupt" fn breakpoint_handler(stack_frame: InterruptStackFrame) {
    log::info!("EXCEPTION: BREAKPOINT\n{:#?}", stack_frame);
}

extern "x86-interrupt" fn double_fault_handler(
    stack_frame: InterruptStackFrame,
    _error_code: u64,
) -> ! {
    panic!("EXCEPTION: DOUBLE FAULT\n{:#?}", stack_frame);
}

extern "x86-interrupt" fn timer_interrupt_handler(_stack_frame: InterruptStackFrame) {
    // Call the timer module's interrupt handler
    crate::time::timer_interrupt();
    
    unsafe {
        PICS.lock()
            .notify_end_of_interrupt(InterruptIndex::Timer.as_u8());
    }
}

extern "x86-interrupt" fn keyboard_interrupt_handler(_stack_frame: InterruptStackFrame) {
    use x86_64::instructions::port::Port;

    let mut port = Port::new(0x60);
    let scancode: u8 = unsafe { port.read() };
    
    // Add scancode to keyboard handler
    crate::keyboard::add_scancode(scancode);

    unsafe {
        PICS.lock()
            .notify_end_of_interrupt(InterruptIndex::Keyboard.as_u8());
    }
}

extern "x86-interrupt" fn divide_by_zero_handler(stack_frame: InterruptStackFrame) {
    log::error!("EXCEPTION: DIVIDE BY ZERO\n{:#?}", stack_frame);
    #[cfg(feature = "test_divide_by_zero")]
    {
        log::info!("TEST_MARKER: DIVIDE_BY_ZERO_HANDLED");
        // For testing, we'll exit cleanly instead of panicking
        crate::test_exit_qemu(crate::QemuExitCode::Success);
    }
    #[cfg(not(feature = "test_divide_by_zero"))]
    panic!("Kernel halted due to divide by zero exception");
}

extern "x86-interrupt" fn invalid_opcode_handler(stack_frame: InterruptStackFrame) {
    log::error!("EXCEPTION: INVALID OPCODE at {:#x}\n{:#?}", 
        stack_frame.instruction_pointer.as_u64(), stack_frame);
    #[cfg(feature = "test_invalid_opcode")]
    {
        log::info!("TEST_MARKER: INVALID_OPCODE_HANDLED");
        crate::test_exit_qemu(crate::QemuExitCode::Success);
    }
    #[cfg(not(feature = "test_invalid_opcode"))]
    loop {
        x86_64::instructions::hlt();
    }
}

extern "x86-interrupt" fn page_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: PageFaultErrorCode,
) {
    use x86_64::registers::control::Cr2;
    
    let accessed_addr = Cr2::read();
    log::error!("EXCEPTION: PAGE FAULT");
    log::error!("Accessed Address: {:?}", accessed_addr);
    log::error!("Error Code: {:?}", error_code);
    log::error!("{:#?}", stack_frame);
    
    #[cfg(feature = "test_page_fault")]
    {
        log::info!("TEST_MARKER: PAGE_FAULT_HANDLED");
        crate::test_exit_qemu(crate::QemuExitCode::Success);
    }
    #[cfg(not(feature = "test_page_fault"))]
    loop {
        x86_64::instructions::hlt();
    }
}

extern "x86-interrupt" fn generic_handler(stack_frame: InterruptStackFrame) {
    log::warn!("UNHANDLED INTERRUPT\n{:#?}", stack_frame);
}