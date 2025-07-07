use crate::gdt;

use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame, PageFaultErrorCode};
use x86_64::VirtAddr;
use pic8259::ChainedPics;
use spin::Once;

mod timer;
pub(crate) mod context_switch;

pub const PIC_1_OFFSET: u8 = 32;
pub const PIC_2_OFFSET: u8 = PIC_1_OFFSET + 8;

pub static PICS: spin::Mutex<ChainedPics> =
    spin::Mutex::new(unsafe { ChainedPics::new(PIC_1_OFFSET, PIC_2_OFFSET) });

#[derive(Debug, Clone, Copy)]
#[repr(u8)]
pub enum InterruptIndex {
    Timer = PIC_1_OFFSET,
    Keyboard,
    // Skip COM2 (IRQ3)
    Serial = PIC_1_OFFSET + 4,  // COM1 is IRQ4
}

/// System call interrupt vector (INT 0x80)
pub const SYSCALL_INTERRUPT_ID: u8 = 0x80;

// Assembly entry points
extern "C" {
    fn syscall_entry();
    fn timer_interrupt_entry();
}

impl InterruptIndex {
    pub fn as_u8(self) -> u8 {
        self as u8
    }

    pub fn as_usize(self) -> usize {
        usize::from(self.as_u8())
    }
}

static IDT: Once<InterruptDescriptorTable> = Once::new();

pub fn init() {
    // Initialize GDT first
    gdt::init();
    // Then initialize IDT
    init_idt();
}

pub fn init_idt() {
    IDT.call_once(|| {
        let mut idt = InterruptDescriptorTable::new();
        
        // CPU exception handlers
        idt.divide_error.set_handler_fn(divide_by_zero_handler);
        
        // Breakpoint handler - must be callable from userspace
        // Set DPL=3 to allow INT3 from Ring 3
        unsafe {
            idt.breakpoint.set_handler_fn(breakpoint_handler)
                .set_privilege_level(x86_64::PrivilegeLevel::Ring3);
        }
        
        idt.invalid_opcode.set_handler_fn(invalid_opcode_handler);
        idt.general_protection_fault.set_handler_fn(general_protection_fault_handler);
        unsafe {
            idt.double_fault.set_handler_fn(double_fault_handler)
                .set_stack_index(gdt::DOUBLE_FAULT_IST_INDEX);
        }
        idt.page_fault.set_handler_fn(page_fault_handler);
        
        // Hardware interrupt handlers
        // Timer interrupt with proper interrupt return path handling
        unsafe {
            idt[InterruptIndex::Timer.as_u8()].set_handler_addr(VirtAddr::new(timer_interrupt_entry as u64));
        }
        idt[InterruptIndex::Keyboard.as_u8()].set_handler_fn(keyboard_interrupt_handler);
        idt[InterruptIndex::Serial.as_u8()].set_handler_fn(serial_interrupt_handler);
        
        // System call handler (INT 0x80)
        // Use assembly handler for proper syscall dispatching
        extern "C" {
            fn syscall_entry();
        }
        unsafe {
            idt[SYSCALL_INTERRUPT_ID].set_handler_addr(x86_64::VirtAddr::new(syscall_entry as u64))
                .set_privilege_level(x86_64::PrivilegeLevel::Ring3);
        }
        log::info!("Syscall handler configured with assembly entry point");
        
        // Set up a generic handler for all unhandled interrupts
        for i in 32..=255 {
            if i != InterruptIndex::Timer.as_u8() 
                && i != InterruptIndex::Keyboard.as_u8() 
                && i != InterruptIndex::Serial.as_u8()
                && i != SYSCALL_INTERRUPT_ID {
                idt[i].set_handler_fn(generic_handler);
            }
        }
        
        idt
    });
    
    let idt = IDT.get().unwrap();
    
    // Log IDT address for debugging
    let idt_ptr = idt as *const _ as u64;
    log::info!("IDT address: {:#x}", idt_ptr);
    
    // Calculate which PML4 entry contains the IDT
    let pml4_index = (idt_ptr >> 39) & 0x1FF;
    log::info!("IDT is in PML4 entry {}", pml4_index);
    
    idt.load();
    log::info!("IDT loaded successfully at {:#x}", idt_ptr);
}

pub fn init_pic() {
    unsafe {
        // Initialize the PIC
        PICS.lock().initialize();
        
        // Unmask timer (IRQ0), keyboard (IRQ1), and serial (IRQ4) interrupts
        use x86_64::instructions::port::Port;
        let mut port: Port<u8> = Port::new(0x21); // PIC1 data port
        let mask = port.read() & !0b00010011; // Clear bit 0 (timer), bit 1 (keyboard), and bit 4 (serial)
        port.write(mask);
    }
}

extern "x86-interrupt" fn breakpoint_handler(stack_frame: InterruptStackFrame) {
    // Check if we came from userspace
    let from_userspace = (stack_frame.code_segment.0 & 3) == 3;
    
    if from_userspace {
        log::info!("BREAKPOINT from USERSPACE at {:#x}", stack_frame.instruction_pointer.as_u64());
        log::info!("Stack: {:#x}, CS: {:?}, SS: {:?}", 
                  stack_frame.stack_pointer.as_u64(),
                  stack_frame.code_segment,
                  stack_frame.stack_segment);
    } else {
        log::info!("EXCEPTION: BREAKPOINT\n{:#?}", stack_frame);
    }
}

extern "x86-interrupt" fn double_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: u64,
) -> ! {
    // Log additional debug info before panicking
    log::error!("DOUBLE FAULT - Error Code: {:#x}", error_code);
    log::error!("Instruction Pointer: {:#x}", stack_frame.instruction_pointer.as_u64());
    log::error!("Stack Pointer: {:#x}", stack_frame.stack_pointer.as_u64());
    log::error!("Code Segment: {:?}", stack_frame.code_segment);
    log::error!("Stack Segment: {:?}", stack_frame.stack_segment);
    
    // Check current page table
    use x86_64::registers::control::Cr3;
    let (frame, _) = Cr3::read();
    log::error!("Current page table frame: {:?}", frame);
    
    panic!("EXCEPTION: DOUBLE FAULT\n{:#?}", stack_frame);
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

extern "x86-interrupt" fn serial_interrupt_handler(_stack_frame: InterruptStackFrame) {
    use x86_64::instructions::port::Port;

    // Read from COM1 data port while data is available
    let mut lsr_port = Port::<u8>::new(0x3F8 + 5); // Line Status Register
    let mut data_port = Port::<u8>::new(0x3F8);    // Data port
    
    // Check if data is available (bit 0 of LSR)
    while unsafe { lsr_port.read() } & 0x01 != 0 {
        let byte = unsafe { data_port.read() };
        crate::serial::add_serial_byte(byte);
    }

    unsafe {
        PICS.lock()
            .notify_end_of_interrupt(InterruptIndex::Serial.as_u8());
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
    
    let accessed_addr = Cr2::read().expect("Failed to read accessed address from CR2");
    
    // Check if this is a guard page access
    if let Some(stack) = crate::memory::stack::is_guard_page_fault(accessed_addr) {
        log::error!("STACK OVERFLOW DETECTED!");
        log::error!("Attempted to access guard page at: {:?}", accessed_addr);
        log::error!("Stack bottom (guard page): {:?}", stack.guard_page());
        log::error!("Stack range: {:?} - {:?}", stack.bottom(), stack.top());
        log::error!("This indicates the stack has overflowed!");
        log::error!("Stack frame: {:#?}", stack_frame);
        
        panic!("Stack overflow - guard page accessed");
    }
    
    log::error!("EXCEPTION: PAGE FAULT");
    log::error!("Accessed Address: {:?}", accessed_addr);
    log::error!("Error Code: {:?}", error_code);
    log::error!("RIP: {:#x}", stack_frame.instruction_pointer.as_u64());
    log::error!("CS: {:#x}", stack_frame.code_segment.0);
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
    // Get the interrupt number from the stack
    // Note: This is a bit hacky but helps with debugging
    let interrupt_num = unsafe {
        // The interrupt number is pushed by the CPU before calling the handler
        // We need to look at the return address to figure out which IDT entry was used
        0 // Placeholder - can't easily get interrupt number in generic handler
    };
    log::warn!("UNHANDLED INTERRUPT from RIP {:#x}", stack_frame.instruction_pointer.as_u64());
    log::warn!("{:#?}", stack_frame);
}

extern "x86-interrupt" fn general_protection_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: u64,
) {
    log::error!("EXCEPTION: GENERAL PROTECTION FAULT");
    log::error!("RIP: {:#x}, CS: {:#x}", stack_frame.instruction_pointer.as_u64(), stack_frame.code_segment.0);
    log::error!("Error Code: {:#x} (selector: {:#x})", error_code, error_code & 0xFFF8);
    
    // Decode error code
    let external = (error_code & 1) != 0;
    let idt = (error_code & 2) != 0;
    let ti = (error_code & 4) != 0;
    let selector_index = (error_code >> 3) & 0x1FFF;
    
    log::error!("  External: {}", external);
    log::error!("  IDT: {} ({})", idt, if idt { "IDT" } else { "GDT/LDT" });
    log::error!("  Table: {} ({})", ti, if ti { "LDT" } else { "GDT" });
    log::error!("  Selector Index: {}", selector_index);
    
    log::error!("{:#?}", stack_frame);
    panic!("General Protection Fault");
}