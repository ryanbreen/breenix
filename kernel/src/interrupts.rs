use crate::gdt;

use pic8259::ChainedPics;
use spin::Once;
use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame, PageFaultErrorCode};
use x86_64::VirtAddr;

pub(crate) mod context_switch;
mod timer;

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
    Serial = PIC_1_OFFSET + 4, // COM1 is IRQ4
}

/// System call interrupt vector (INT 0x80)
pub const SYSCALL_INTERRUPT_ID: u8 = 0x80;

// Assembly entry points
extern "C" {
    #[allow(dead_code)]
    fn syscall_entry();
    #[allow(dead_code)]
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
        
        // Debug exception handler (#DB) - IDT[1]
        // Triggered by TF (Trap Flag) for single-stepping
        idt.debug.set_handler_fn(debug_handler);

        // Breakpoint handler - must be callable from userspace
        // Set DPL=3 to allow INT3 from Ring 3
        // Use assembly entry point for proper swapgs handling
        extern "C" {
            fn breakpoint_entry();
        }
        unsafe {
            let breakpoint_entry_addr = breakpoint_entry as u64;
            idt.breakpoint
                .set_handler_addr(VirtAddr::new(breakpoint_entry_addr))
                .set_privilege_level(x86_64::PrivilegeLevel::Ring3);
        }

        idt.invalid_opcode.set_handler_fn(invalid_opcode_handler);
        idt.general_protection_fault
            .set_handler_fn(general_protection_fault_handler);
        idt.stack_segment_fault
            .set_handler_fn(stack_segment_fault_handler);
        unsafe {
            idt.double_fault
                .set_handler_fn(double_fault_handler)
                .set_stack_index(gdt::DOUBLE_FAULT_IST_INDEX);
        }
        unsafe {
            idt.page_fault
                .set_handler_fn(page_fault_handler)
                .set_stack_index(gdt::PAGE_FAULT_IST_INDEX);
        }

        // Hardware interrupt handlers
        // Timer interrupt with proper interrupt return path handling
        // CRITICAL: Use high-half alias for timer entry so it remains accessible after CR3 switch
        extern "C" {
            fn timer_interrupt_entry();
        }
        unsafe {
            // Convert low-half address to high-half alias
            let timer_entry_low = timer_interrupt_entry as u64;
            
            // CRITICAL: Validate the address is in expected range before conversion
            if timer_entry_low < 0x100000 || timer_entry_low > 0x40000000 {
                log::error!("INVALID timer_interrupt_entry address: {:#x}", timer_entry_low);
                // For now, use the low address directly - it should work since we preserve PML4[0]
                log::warn!("Using low-half address for timer entry (temporary workaround)");
                idt[InterruptIndex::Timer.as_u8()]
                    .set_handler_addr(VirtAddr::new(timer_entry_low));
            } else {
                let timer_entry_high = crate::memory::layout::high_alias_from_low(timer_entry_low);
                log::info!("Timer entry: low={:#x} -> high={:#x}", timer_entry_low, timer_entry_high);
                idt[InterruptIndex::Timer.as_u8()]
                    .set_handler_addr(VirtAddr::new(timer_entry_high));
            }
        }
        idt[InterruptIndex::Keyboard.as_u8()].set_handler_fn(keyboard_interrupt_handler);
        idt[InterruptIndex::Serial.as_u8()].set_handler_fn(serial_interrupt_handler);

        // System call handler (INT 0x80)
        // Use assembly handler for proper syscall dispatching
        // CRITICAL: Use high-half alias for syscall entry so it remains accessible from userspace
        extern "C" {
            fn syscall_entry();
        }
        unsafe {
            // Convert low-half address to high-half alias
            let syscall_entry_low = syscall_entry as u64;
            
            // CRITICAL: Validate the address is in expected range before conversion
            if syscall_entry_low < 0x100000 || syscall_entry_low > 0x40000000 {
                log::error!("INVALID syscall_entry address: {:#x}", syscall_entry_low);
                // For now, use the low address directly - it should work since we preserve PML4[0]
                log::warn!("Using low-half address for syscall entry (temporary workaround)");
                idt[SYSCALL_INTERRUPT_ID]
                    .set_handler_addr(x86_64::VirtAddr::new(syscall_entry_low))
                    .set_privilege_level(x86_64::PrivilegeLevel::Ring3);
            } else {
                let syscall_entry_high = crate::memory::layout::high_alias_from_low(syscall_entry_low);
                log::info!("Syscall entry: low={:#x} -> high={:#x}", syscall_entry_low, syscall_entry_high);
                idt[SYSCALL_INTERRUPT_ID]
                    .set_handler_addr(x86_64::VirtAddr::new(syscall_entry_high))
                    .set_privilege_level(x86_64::PrivilegeLevel::Ring3);
            }
        }
        
        // Log IDT gate attributes for verification
        log::info!("IDT[0x80] gate attributes:");
        let actual_syscall_addr = syscall_entry as u64;
        if actual_syscall_addr < 0x100000 || actual_syscall_addr > 0x40000000 {
            log::info!("  Handler address: {:#x} (low-half, validation failed)", actual_syscall_addr);
        } else {
            let syscall_entry_high = crate::memory::layout::high_alias_from_low(actual_syscall_addr);
            log::info!("  Handler address: {:#x} (high-half alias)", syscall_entry_high);
        }
        log::info!("  DPL (privilege level): Ring3 (allowing userspace access)");
        log::info!("  Gate type: Interrupt gate (interrupts disabled on entry)");
        log::info!("Syscall handler configured with assembly entry point");

        // Set up a generic handler for all unhandled interrupts
        for i in 32..=255 {
            if i != InterruptIndex::Timer.as_u8()
                && i != InterruptIndex::Keyboard.as_u8()
                && i != InterruptIndex::Serial.as_u8()
                && i != SYSCALL_INTERRUPT_ID
            {
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

extern "x86-interrupt" fn debug_handler(stack_frame: InterruptStackFrame) {
    // Enter exception context - use preempt_disable for exceptions (not IRQs)
    crate::per_cpu::preempt_disable();
    
    // Check if we came from userspace
    let from_userspace = (stack_frame.code_segment.0 & 3) == 3;

    if from_userspace {
        log::info!("🎯 #DB (DEBUG EXCEPTION) from USERSPACE - IRETQ SUCCEEDED!");
        log::info!(
            "  RIP: {:#x} (first user instruction after IRETQ)",
            stack_frame.instruction_pointer.as_u64()
        );
        log::info!(
            "  RSP: {:#x}, CS: {:#x} (RPL={}), SS: {:#x}",
            stack_frame.stack_pointer.as_u64(),
            stack_frame.code_segment.0,
            stack_frame.code_segment.0 & 3,
            stack_frame.stack_segment.0
        );
        // TODO: Clear TF flag to stop single-stepping after proving IRETQ works
    } else {
        log::info!("#DB (Debug Exception) from kernel at {:#x}", 
                  stack_frame.instruction_pointer.as_u64());
    }
    
    // Decrement preempt count on exception exit
    crate::per_cpu::preempt_enable();
}

/// Rust breakpoint handler called from assembly entry point
/// This version is called with swapgs already handled
#[no_mangle]
pub extern "C" fn rust_breakpoint_handler(frame_ptr: *mut u64) {
    // Note: CLI and swapgs already handled by assembly entry
    // No need to disable interrupts here
    
    // Raw serial output FIRST to confirm we're in BP handler
    unsafe {
        core::arch::asm!(
            "mov dx, 0x3F8",
            "mov al, 0x42",      // 'B' for Breakpoint
            "out dx, al",
            "mov al, 0x50",      // 'P' for bP
            "out dx, al",
            options(nostack, nomem, preserves_flags)
        );
    }
    
    // Use serial_println first - it might work even if log doesn't
    crate::serial_println!("BP_HANDLER_ENTRY!");
    
    // Enter exception context - use preempt_disable for exceptions (not IRQs)
    crate::serial_println!("About to call preempt_disable from BP handler");
    crate::per_cpu::preempt_disable();
    crate::serial_println!("Called preempt_disable from BP handler");
    
    // Parse the frame structure
    // Frame layout: [r15,r14,...,rax,error_code,RIP,CS,RFLAGS,RSP,SS]
    unsafe {
        let frame = frame_ptr;
        let rip_ptr = frame.offset(16);  // Skip 15 regs + error code
        let cs_ptr = frame.offset(17);
        let _rflags_ptr = frame.offset(18);
        let rsp_ptr = frame.offset(19);
        let _ss_ptr = frame.offset(20);
        
        let rip = *rip_ptr;
        let cs = *cs_ptr;
        let rsp = *rsp_ptr;
        
        // CRITICAL: Do NOT advance RIP manually - CPU already advanced past INT3
        // The saved RIP already points to the instruction after the breakpoint
        
        // Check if we came from userspace
        let from_userspace = (cs & 3) == 3;
        
        crate::serial_println!("BP from_userspace={}, CS={:#x}", from_userspace, cs);

        if from_userspace {
            // Raw serial output for userspace breakpoint - SUCCESS!
            core::arch::asm!(
                "mov dx, 0x3F8",
                "mov al, 0x55",      // 'U' for Userspace
                "out dx, al",
                "mov al, 0x33",      // '3' for Ring 3
                "out dx, al",
                "mov al, 0x21",      // '!' for success
                "out dx, al",
                options(nostack, nomem, preserves_flags)
            );
            
            // Use only serial output to avoid framebuffer issues
            crate::serial_println!("🎉 BREAKPOINT from USERSPACE - Ring 3 SUCCESS!");
            crate::serial_println!("  RIP: {:#x}, CS: {:#x} (RPL={})", rip, cs, cs & 3);
            crate::serial_println!("  RSP: {:#x}", rsp);
        } else {
            log::debug!("Breakpoint from kernel at RIP: {:#x}", rip);
        }
    }
    
    // Decrement preempt count on exception exit
    crate::serial_println!("BP handler: About to call preempt_enable");
    crate::per_cpu::preempt_enable();
    crate::serial_println!("BP handler: Called preempt_enable, exiting handler");
}

extern "x86-interrupt" fn double_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: u64,
) -> ! {
    // DIAGNOSTIC OUTPUT AT THE VERY START
    let cr2: u64;
    let cr3: u64;
    let actual_rsp: u64;
    unsafe {
        use x86_64::registers::control::{Cr2, Cr3};
        cr2 = Cr2::read().unwrap_or(x86_64::VirtAddr::zero()).as_u64();
        let (frame, _) = Cr3::read();
        cr3 = frame.start_address().as_u64();
        core::arch::asm!("mov {}, rsp", out(reg) actual_rsp);
    }

    crate::serial_println!("[DIAG:DOUBLEFAULT] ==============================");
    crate::serial_println!("[DIAG:DOUBLEFAULT] Error code: {:#x}", error_code);
    crate::serial_println!("[DIAG:DOUBLEFAULT] RIP: {:#x}", stack_frame.instruction_pointer.as_u64());
    crate::serial_println!("[DIAG:DOUBLEFAULT] CS: {:#x}", stack_frame.code_segment.0);
    crate::serial_println!("[DIAG:DOUBLEFAULT] RFLAGS: {:#x}", stack_frame.cpu_flags.bits());
    crate::serial_println!("[DIAG:DOUBLEFAULT] RSP (frame): {:#x}", stack_frame.stack_pointer.as_u64());
    crate::serial_println!("[DIAG:DOUBLEFAULT] RSP (actual): {:#x}", actual_rsp);
    crate::serial_println!("[DIAG:DOUBLEFAULT] SS: {:#x}", stack_frame.stack_segment.0);
    crate::serial_println!("[DIAG:DOUBLEFAULT] CR2: {:#x}", cr2);
    crate::serial_println!("[DIAG:DOUBLEFAULT] CR3: {:#x}", cr3);
    crate::serial_println!("[DIAG:DOUBLEFAULT] ==============================");

    // Raw serial output FIRST to confirm we're in DF handler
    unsafe {
        core::arch::asm!(
            "mov dx, 0x3F8",
            "mov al, 0x44",      // 'D' for Double Fault
            "out dx, al",
            "mov al, 0x46",      // 'F'
            "out dx, al",
            options(nostack, nomem, preserves_flags)
        );
    }
    
    // Log comprehensive debug info before panicking
    log::error!("==================== DOUBLE FAULT ====================");
    log::error!("CR2 (faulting address): {:#x}", cr2);
    log::error!("Error Code: {:#x}", error_code);
    log::error!("RIP: {:#x}", stack_frame.instruction_pointer.as_u64());
    log::error!("CS: {:?}", stack_frame.code_segment);
    log::error!("RFLAGS: {:?}", stack_frame.cpu_flags);
    log::error!("RSP (from frame): {:#x}", stack_frame.stack_pointer.as_u64());
    log::error!("SS: {:?}", stack_frame.stack_segment);
    log::error!("Actual RSP (current): {:#x}", actual_rsp);
    
    // Check current page table
    use x86_64::registers::control::Cr3;
    let (frame, _) = Cr3::read();
    log::error!("Current CR3: {:#x}", frame.start_address().as_u64());
    
    // Analyze the fault
    if cr2 != 0 {
        log::error!("Likely caused by page fault at {:#x}", cr2);
        
        // Check if it's a stack access
        if cr2 >= actual_rsp.saturating_sub(0x1000) && cr2 <= actual_rsp.saturating_add(0x1000) {
            log::error!(">>> Fault appears to be a STACK ACCESS near RSP");
        }
    }
    log::error!("======================================================");

    panic!("EXCEPTION: DOUBLE FAULT\n{:#?}", stack_frame);
}

extern "x86-interrupt" fn keyboard_interrupt_handler(_stack_frame: InterruptStackFrame) {
    use x86_64::instructions::port::Port;

    // Enter hardware IRQ context
    crate::per_cpu::irq_enter();

    let mut port = Port::new(0x60);
    let scancode: u8 = unsafe { port.read() };

    // Add scancode to keyboard handler
    crate::keyboard::add_scancode(scancode);

    unsafe {
        PICS.lock()
            .notify_end_of_interrupt(InterruptIndex::Keyboard.as_u8());
    }
    
    // Exit hardware IRQ context
    crate::per_cpu::irq_exit();
}

extern "x86-interrupt" fn serial_interrupt_handler(_stack_frame: InterruptStackFrame) {
    use x86_64::instructions::port::Port;

    // Enter hardware IRQ context
    crate::per_cpu::irq_enter();

    // Read from COM1 data port while data is available
    let mut lsr_port = Port::<u8>::new(0x3F8 + 5); // Line Status Register
    let mut data_port = Port::<u8>::new(0x3F8); // Data port

    // Check if data is available (bit 0 of LSR)
    while unsafe { lsr_port.read() } & 0x01 != 0 {
        let byte = unsafe { data_port.read() };
        crate::serial::add_serial_byte(byte);
    }

    unsafe {
        PICS.lock()
            .notify_end_of_interrupt(InterruptIndex::Serial.as_u8());
    }
    
    // Exit hardware IRQ context
    crate::per_cpu::irq_exit();
}

extern "x86-interrupt" fn divide_by_zero_handler(stack_frame: InterruptStackFrame) {
    // Increment preempt count on exception entry
    crate::per_cpu::preempt_disable();
    
    log::error!("EXCEPTION: DIVIDE BY ZERO\n{:#?}", stack_frame);
    #[cfg(feature = "test_divide_by_zero")]
    {
        log::info!("TEST_MARKER: DIVIDE_BY_ZERO_HANDLED");
        // For testing, we'll exit cleanly instead of panicking
        crate::test_exit_qemu(crate::QemuExitCode::Success);
    }
    #[cfg(not(feature = "test_divide_by_zero"))]
    {
        // Decrement preempt count before panic
            crate::per_cpu::preempt_enable();
        panic!("Kernel halted due to divide by zero exception");
    }
}

extern "x86-interrupt" fn invalid_opcode_handler(stack_frame: InterruptStackFrame) {
    // Increment preempt count on exception entry
    crate::per_cpu::preempt_disable();
    
    log::error!(
        "EXCEPTION: INVALID OPCODE at {:#x}\n{:#?}",
        stack_frame.instruction_pointer.as_u64(),
        stack_frame
    );
    #[cfg(feature = "test_invalid_opcode")]
    {
        log::info!("TEST_MARKER: INVALID_OPCODE_HANDLED");
        crate::test_exit_qemu(crate::QemuExitCode::Success);
    }
    #[cfg(not(feature = "test_invalid_opcode"))]
    loop {
        x86_64::instructions::hlt();
    }
    
    // Note: preempt_enable() not called here since we enter infinite loop or exit
}

extern "x86-interrupt" fn page_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: PageFaultErrorCode,
) {
    use x86_64::registers::control::Cr2;

    // DIAGNOSTIC OUTPUT AT THE VERY START
    let cr2 = Cr2::read().unwrap_or(x86_64::VirtAddr::zero()).as_u64();
    let cr3 = {
        use x86_64::registers::control::Cr3;
        let (frame, _) = Cr3::read();
        frame.start_address().as_u64()
    };

    crate::serial_println!("[DIAG:PAGEFAULT] ==============================");
    crate::serial_println!("[DIAG:PAGEFAULT] Fault addr: {:#x}", cr2);
    crate::serial_println!("[DIAG:PAGEFAULT] Error code: {:#x}", error_code.bits());
    crate::serial_println!("[DIAG:PAGEFAULT] RIP: {:#x}", stack_frame.instruction_pointer.as_u64());
    crate::serial_println!("[DIAG:PAGEFAULT] CS: {:#x}", stack_frame.code_segment.0);
    crate::serial_println!("[DIAG:PAGEFAULT] RFLAGS: {:#x}", stack_frame.cpu_flags.bits());
    crate::serial_println!("[DIAG:PAGEFAULT] RSP: {:#x}", stack_frame.stack_pointer.as_u64());
    crate::serial_println!("[DIAG:PAGEFAULT] SS: {:#x}", stack_frame.stack_segment.0);
    crate::serial_println!("[DIAG:PAGEFAULT] CR3: {:#x}", cr3);
    crate::serial_println!("[DIAG:PAGEFAULT] ==============================");

    // Increment preempt count on exception entry FIRST to avoid recursion
    crate::per_cpu::preempt_disable();

    let accessed_addr = Cr2::read().expect("Failed to read accessed address from CR2");
    
    // Use raw serial output for critical info to avoid recursion
    unsafe {
        // Output 'P' for page fault
        core::arch::asm!(
            "mov dx, 0x3F8",
            "mov al, 0x50",      // 'P'
            "out dx, al",
            options(nostack, nomem, preserves_flags)
        );
        
        // Output 'F' for fault
        core::arch::asm!(
            "mov dx, 0x3F8",
            "mov al, 0x46",      // 'F'
            "out dx, al",
            options(nostack, nomem, preserves_flags)
        );
        
        // Check error code bits
        let error_bits = error_code.bits();
        if error_bits & 1 == 0 {
            // Not present
            core::arch::asm!(
                "mov dx, 0x3F8",
                "mov al, 0x30",      // '0' for not present
                "out dx, al",
                options(nostack, nomem, preserves_flags)
            );
        } else {
            // Protection violation
            core::arch::asm!(
                "mov dx, 0x3F8",
                "mov al, 0x31",      // '1' for protection
                "out dx, al",
                options(nostack, nomem, preserves_flags)
            );
        }
        
        // Check if fault is at 0x400000 (our int3 page)
        if accessed_addr.as_u64() == 0x400000 {
            core::arch::asm!(
                "mov dx, 0x3F8",
                "mov al, 0x34",      // '4' for 0x400000
                "out dx, al",
                options(nostack, nomem, preserves_flags)
            );
        } else if accessed_addr.as_u64() >= 0x800000 && accessed_addr.as_u64() < 0x900000 {
            core::arch::asm!(
                "mov dx, 0x3F8",
                "mov al, 0x38",      // '8' for stack area
                "out dx, al",
                options(nostack, nomem, preserves_flags)
            );
        } else {
            core::arch::asm!(
                "mov dx, 0x3F8",
                "mov al, 0x3F",      // '?' for other
                "out dx, al",
                options(nostack, nomem, preserves_flags)
            );
        }
    }
    
    // Emergency output to confirm we're in page fault handler  
    crate::serial_println!("PF_ENTRY!");
    
    // Output page fault error code details
    let error_bits = error_code.bits();
    crate::serial_println!("PF @ {:#x} Error: {:#x} (P={}, W={}, U={}, I={})", 
        accessed_addr.as_u64(),
        error_bits,
        if error_code.contains(PageFaultErrorCode::PROTECTION_VIOLATION) { 1 } else { 0 },
        if error_code.contains(PageFaultErrorCode::CAUSED_BY_WRITE) { 1 } else { 0 },
        if error_code.contains(PageFaultErrorCode::USER_MODE) { 1 } else { 0 },
        if error_code.contains(PageFaultErrorCode::INSTRUCTION_FETCH) { 1 } else { 0 }
    );
    
    // Quick debug output for int3 test - use raw output
    unsafe {
        // Output 'F' for Fault
        core::arch::asm!(
            "mov dx, 0x3F8",
            "mov al, 0x46",      // 'F'
            "out dx, al",
            options(nostack, nomem, preserves_flags)
        );
        
        // Check if it's 0x400000 (our int3 page)
        if accessed_addr.as_u64() == 0x400000 {
            // Output '4' to indicate fault at 0x400000
            core::arch::asm!(
                "mov dx, 0x3F8",
                "mov al, 0x34",      // '4'
                "out dx, al",
                options(nostack, nomem, preserves_flags)
            );
        }
    }
    
    // Check if this came from userspace
    let from_userspace = (stack_frame.code_segment.0 & 3) == 3;

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

    crate::serial_println!("EXCEPTION: PAGE FAULT - Now using IST stack for reliable diagnostics");
    
    // CRITICAL: Enhanced diagnostics for CR3 switch debugging
    unsafe {
        use x86_64::registers::control::Cr3;
        let (current_cr3, _flags) = Cr3::read();
        let rsp: u64;
        let rbp: u64;
        let _rflags: u64;
        core::arch::asm!("mov {}, rsp", out(reg) rsp);
        core::arch::asm!("mov {}, rbp", out(reg) rbp);
        core::arch::asm!("pushfq; pop {}", out(reg) _rflags);
        
        crate::serial_println!("CR3 SWITCH DEBUG:");
        crate::serial_println!("  Current CR3: {:#x}", current_cr3.start_address().as_u64());
        crate::serial_println!("  CR2 (fault addr): {:#x}", accessed_addr.as_u64());
        crate::serial_println!("  Error code: {:#x} (P={} W={} U={} I={} PK={})",
            error_code.bits(),
            if error_code.contains(PageFaultErrorCode::PROTECTION_VIOLATION) { 1 } else { 0 },
            if error_code.contains(PageFaultErrorCode::CAUSED_BY_WRITE) { 1 } else { 0 },
            if error_code.contains(PageFaultErrorCode::USER_MODE) { 1 } else { 0 },
            if error_code.contains(PageFaultErrorCode::INSTRUCTION_FETCH) { 1 } else { 0 },
            if error_code.contains(PageFaultErrorCode::PROTECTION_KEY) { 1 } else { 0 }
        );
        crate::serial_println!("  CS:RIP: {:#x}:{:#x}", stack_frame.code_segment.0, stack_frame.instruction_pointer.as_u64());
        crate::serial_println!("  SS:RSP: {:#x}:{:#x}", stack_frame.stack_segment.0, stack_frame.stack_pointer.as_u64());
        crate::serial_println!("  RFLAGS: {:#x}", stack_frame.cpu_flags.bits());
        crate::serial_println!("  Current RSP: {:#x}, RBP: {:#x}", rsp, rbp);
        
        // Determine what PML4 entry the fault address belongs to
        let pml4_index = (accessed_addr.as_u64() >> 39) & 0x1FF;
        crate::serial_println!("  Fault address PML4 index: {} (PML4[{}])", pml4_index, pml4_index);
        
        // Also log which PML4 entry the faulting instruction belongs to
        let rip_pml4_index = (stack_frame.instruction_pointer.as_u64() >> 39) & 0x1FF;
        crate::serial_println!("  RIP address PML4 index: {} (PML4[{}])", rip_pml4_index, rip_pml4_index);
        
        // Check if this is instruction fetch vs data access
        if error_code.contains(PageFaultErrorCode::INSTRUCTION_FETCH) {
            crate::serial_println!("  INSTRUCTION FETCH fault - code page not executable or not present!");
        } else if error_code.contains(PageFaultErrorCode::CAUSED_BY_WRITE) {
            crate::serial_println!("  WRITE fault - page not writable or not present!");
        } else {
            crate::serial_println!("  READ fault - page not readable or not present!");
        }
    }
    
    // Enhanced logging for userspace faults (Ring 3 privilege violation tests)
    if from_userspace {
        log::error!("✓ PAGE FAULT from USERSPACE (Ring 3 privilege test detected)");
        log::error!("  CR2 (accessed address): {:#x}", accessed_addr.as_u64());
        log::error!("  Error code: {:#x}", error_code.bits());
        log::error!("    U={} ({})", 
            if error_code.contains(PageFaultErrorCode::USER_MODE) { 1 } else { 0 },
            if error_code.contains(PageFaultErrorCode::USER_MODE) { "from userspace" } else { "from kernel" }
        );
        log::error!("    P={} ({})",
            if error_code.contains(PageFaultErrorCode::PROTECTION_VIOLATION) { 1 } else { 0 },
            if error_code.contains(PageFaultErrorCode::PROTECTION_VIOLATION) { "protection violation" } else { "not present" }
        );
        log::error!("  CS: {:#x} (RPL={})", stack_frame.code_segment.0, stack_frame.code_segment.0 & 3);
        log::error!("  RIP: {:#x}", stack_frame.instruction_pointer.as_u64());
        
        // Check if this is an expected test fault
        if accessed_addr.as_u64() == 0x50000000 {
            log::info!("  ✓ This is the expected unmapped memory test (0x50000000)");
        }
    } else {
        log::error!("Accessed Address: {:?}", accessed_addr);
        log::error!("Error Code: {:?}", error_code);
        log::error!("RIP: {:#x}", stack_frame.instruction_pointer.as_u64());
        log::error!("CS: {:#x}", stack_frame.code_segment.0);
    }
    
    log::error!("{:#?}", stack_frame);

    #[cfg(feature = "test_page_fault")]
    {
        log::info!("TEST_MARKER: PAGE_FAULT_HANDLED");
        crate::test_exit_qemu(crate::QemuExitCode::Success);
    }
    #[cfg(not(feature = "test_page_fault"))]
    {
        // For userspace faults, terminate the process and schedule another
        if from_userspace {
            log::error!("Terminating faulting userspace process and scheduling next...");

            // Find the process by CR3 - this is more reliable than using current_thread_id
            // because during context switch the "current" thread may not match the faulting process
            let mut faulting_thread_id: Option<u64> = None;

            crate::process::with_process_manager(|pm| {
                if let Some((pid, process)) = pm.find_process_by_cr3_mut(cr3) {
                    let name = process.name.clone();
                    // Get the thread ID before we exit the process
                    faulting_thread_id = process.main_thread.as_ref().map(|t| t.id);
                    log::error!("Killing process {} (PID {}) due to page fault (CR3={:#x})",
                        name, pid.as_u64(), cr3);
                    pm.exit_process(pid, -11); // SIGSEGV exit code
                } else {
                    log::error!("Could not find process with CR3={:#x} - cannot terminate", cr3);
                }
            });

            // Mark thread as terminated by setting it not runnable
            if let Some(thread_id) = faulting_thread_id {
                crate::task::scheduler::with_thread_mut(thread_id, |thread| {
                    thread.state = crate::task::thread::ThreadState::Terminated;
                });
            }

            // Re-enable preemption before scheduling
            crate::per_cpu::preempt_enable();

            // Force a reschedule to pick up the next thread
            crate::task::scheduler::set_need_resched();

            log::info!("About to schedule next thread after killing faulting process...");

            // Switch CR3 back to kernel page table
            unsafe {
                use x86_64::registers::control::Cr3;
                use x86_64::structures::paging::PhysFrame;
                let kernel_cr3 = crate::per_cpu::get_kernel_cr3();
                if kernel_cr3 != 0 {
                    log::info!("Switching to kernel CR3: {:#x}", kernel_cr3);
                    Cr3::write(
                        PhysFrame::containing_address(x86_64::PhysAddr::new(kernel_cr3)),
                        Cr3::read().1,
                    );
                }
            }

            // Set a flag that makes this context schedulable BEFORE enabling interrupts
            // This tells can_schedule() that we're in exception cleanup and can be preempted
            crate::per_cpu::set_exception_cleanup_context();

            // Now enable interrupts so timer can fire and trigger scheduling
            x86_64::instructions::interrupts::enable();

            loop {
                x86_64::instructions::hlt();
            }
        }

        // Kernel page fault - this is a bug, halt
        loop {
            x86_64::instructions::hlt();
        }
    }

    // Note: preempt_enable() not called here since we enter infinite loop or exit
}

extern "x86-interrupt" fn generic_handler(stack_frame: InterruptStackFrame) {
    // Enter hardware IRQ context for unknown interrupts
    crate::per_cpu::irq_enter();
    
    // Get the interrupt number from the stack
    // Note: This is a bit hacky but helps with debugging
    let _interrupt_num = {
        // The interrupt number is pushed by the CPU before calling the handler
        // We need to look at the return address to figure out which IDT entry was used
        0 // Placeholder - can't easily get interrupt number in generic handler
    };
    log::warn!(
        "UNHANDLED INTERRUPT from RIP {:#x}",
        stack_frame.instruction_pointer.as_u64()
    );
    log::warn!("{:#?}", stack_frame);
    
    // Exit hardware IRQ context
    crate::per_cpu::irq_exit();
}

extern "x86-interrupt" fn stack_segment_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: u64,
) {
    // Increment preempt count on exception entry
    crate::per_cpu::preempt_disable();
    
    // Check if this came from userspace
    let from_userspace = (stack_frame.code_segment.0 & 3) == 3;
    
    log::error!("EXCEPTION: STACK SEGMENT FAULT (#SS)");
    log::error!("  Error Code: {:#x}", error_code);
    
    // #SS during IRETQ is usually due to invalid SS selector or stack issues
    if !from_userspace {
        log::error!("  💥 LIKELY IRETQ FAILURE - invalid SS selector or stack!");
        log::error!("  Check: SS selector validity, DPL=3, stack mapping");
    }
    
    log::error!("  CS: {:#x} (RPL={})", stack_frame.code_segment.0, stack_frame.code_segment.0 & 3);
    log::error!("  RIP: {:#x}", stack_frame.instruction_pointer.as_u64());
    log::error!("  RSP: {:#x}", stack_frame.stack_pointer.as_u64());
    log::error!("  SS: {:#x}", stack_frame.stack_segment.0);
    
    log::error!("\n{:#?}", stack_frame);
    panic!("Stack segment fault - likely IRETQ issue!");
}

extern "x86-interrupt" fn general_protection_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: u64,
) {
    // DIAGNOSTIC OUTPUT AT THE VERY START
    let cr3 = {
        use x86_64::registers::control::Cr3;
        let (frame, _) = Cr3::read();
        frame.start_address().as_u64()
    };

    crate::serial_println!("[DIAG:GPF] ==============================");
    crate::serial_println!("[DIAG:GPF] Error code: {:#x}", error_code);
    crate::serial_println!("[DIAG:GPF] RIP: {:#x}", stack_frame.instruction_pointer.as_u64());
    crate::serial_println!("[DIAG:GPF] CS: {:#x}", stack_frame.code_segment.0);
    crate::serial_println!("[DIAG:GPF] RFLAGS: {:#x}", stack_frame.cpu_flags.bits());
    crate::serial_println!("[DIAG:GPF] RSP: {:#x}", stack_frame.stack_pointer.as_u64());
    crate::serial_println!("[DIAG:GPF] SS: {:#x}", stack_frame.stack_segment.0);
    crate::serial_println!("[DIAG:GPF] CR3: {:#x}", cr3);
    crate::serial_println!("[DIAG:GPF] ==============================");

    // Raw serial output FIRST to confirm we're in GP handler
    unsafe {
        core::arch::asm!(
            "mov dx, 0x3F8",
            "mov al, 0x47",      // 'G' for GP fault
            "out dx, al",
            "mov al, 0x50",      // 'P'
            "out dx, al",
            options(nostack, nomem, preserves_flags)
        );
    }

    // Increment preempt count on exception entry
    crate::per_cpu::preempt_disable();
    
    // Check if this came from userspace
    let from_userspace = (stack_frame.code_segment.0 & 3) == 3;
    
    log::error!("EXCEPTION: GENERAL PROTECTION FAULT (#GP)");
    
    // Decode the error code to identify the problematic selector
    let external = (error_code & 1) != 0;
    let table = (error_code >> 1) & 0b11;
    let index = (error_code >> 3) & 0x1FFF;
    
    let table_name = match table {
        0b00 => "GDT",
        0b01 => "IDT", 
        0b10 => "LDT",
        0b11 => "IDT",
        _ => "???",
    };
    
    let selector = (index << 3) | ((table & 1) << 2) | (if from_userspace { 3 } else { 0 });
    
    log::error!("  Error Code: {:#x}", error_code);
    log::error!("  Decoded: external={}, table={} ({}), index={}, selector={:#x}",
               external, table, table_name, index, selector);
    
    // Check if this might be an IRETQ failure
    if !from_userspace && stack_frame.instruction_pointer.as_u64() < 0x1000_0000 {
        log::error!("  💥 LIKELY IRETQ FAILURE - fault during return to userspace!");
        log::error!("  Problematic selector: {:#x} from {}", selector, table_name);
        if selector == 0x33 {
            log::error!("  Issue with user CS (0x33) - check GDT entry, L bit, DPL");
        } else if selector == 0x2b {
            log::error!("  Issue with user SS (0x2b) - check GDT entry, DPL");
        }
    }
    
    log::error!("  CS: {:#x} (RPL={})", stack_frame.code_segment.0, stack_frame.code_segment.0 & 3);
    log::error!("  RIP: {:#x}", stack_frame.instruction_pointer.as_u64());
    
    // Enhanced logging for userspace GPFs (Ring 3 privilege violation tests)
    if from_userspace {
        log::error!("  GPF from USERSPACE (Ring 3)");
        
        // Try to identify which instruction caused the fault
        {
            let rip = stack_frame.instruction_pointer.as_u64() as *const u8;
            let byte = unsafe { core::ptr::read_volatile(rip) };
                match byte {
                    0xfa => log::info!("  ✓ CLI instruction detected (0xfa) - expected privilege violation"),
                    0xf4 => log::info!("  ✓ HLT instruction detected (0xf4) - expected privilege violation"),
                    0x0f => {
                        // Check for MOV CR3 (0x0f 0x22 0xd8)
                        let byte2 = unsafe { core::ptr::read_volatile(rip.offset(1)) };
                        if byte2 == 0x22 {
                            log::info!("  ✓ MOV CR3 instruction detected (0x0f 0x22) - expected privilege violation");
                        }
                    },
                    _ => log::debug!("  Instruction byte at fault: {:#02x}", byte),
                }
        }
    } else {
        log::error!(
            "RIP: {:#x}, CS: {:#x}",
            stack_frame.instruction_pointer.as_u64(),
            stack_frame.code_segment.0
        );
        log::error!(
            "Error Code: {:#x} (selector: {:#x})",
            error_code,
            error_code & 0xFFF8
        );
    }

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

    // Handle userspace GPFs gracefully by terminating the process
    if from_userspace {
        log::error!("Terminating faulting userspace process due to GPF...");

        // Find the process by CR3
        let mut faulting_thread_id: Option<u64> = None;

        crate::process::with_process_manager(|pm| {
            if let Some((pid, process)) = pm.find_process_by_cr3_mut(cr3) {
                let name = process.name.clone();
                // Get the thread ID before we exit the process
                faulting_thread_id = process.main_thread.as_ref().map(|t| t.id);
                log::error!("Killing process {} (PID {}) due to GPF (CR3={:#x})",
                    name, pid.as_u64(), cr3);
                pm.exit_process(pid, -11); // SIGSEGV exit code
            } else {
                log::error!("Could not find process with CR3={:#x} - cannot terminate", cr3);
            }
        });

        // Mark thread as terminated by setting it not runnable
        if let Some(thread_id) = faulting_thread_id {
            crate::task::scheduler::with_thread_mut(thread_id, |thread| {
                thread.state = crate::task::thread::ThreadState::Terminated;
            });
        }

        // Re-enable preemption before scheduling
        crate::per_cpu::preempt_enable();

        // Force a reschedule to pick up the next thread
        crate::task::scheduler::set_need_resched();

        log::info!("About to schedule next thread after killing faulting process...");

        // Switch CR3 back to kernel page table
        unsafe {
            use x86_64::registers::control::Cr3;
            use x86_64::structures::paging::PhysFrame;
            let kernel_cr3 = crate::per_cpu::get_kernel_cr3();
            if kernel_cr3 != 0 {
                log::info!("Switching to kernel CR3: {:#x}", kernel_cr3);
                Cr3::write(
                    PhysFrame::containing_address(x86_64::PhysAddr::new(kernel_cr3)),
                    Cr3::read().1,
                );
            }
        }

        // Set exception cleanup context flag
        crate::per_cpu::set_exception_cleanup_context();

        // Enable interrupts so timer can fire and trigger scheduling
        x86_64::instructions::interrupts::enable();

        // Halt until next interrupt
        loop {
            x86_64::instructions::hlt();
        }
    }

    // Kernel GPF - this is a bug, panic
    crate::per_cpu::preempt_enable();
    panic!("General Protection Fault");
}

/// Get IDT base and limit for logging
pub fn get_idt_info() -> (u64, u16) {
    let idtr = x86_64::instructions::tables::sidt();
    (idtr.base.as_u64(), idtr.limit)
}

/// Validate that the IDT entry for the timer interrupt is properly configured
/// Returns (is_valid, handler_address, description)
pub fn validate_timer_idt_entry() -> (bool, u64, &'static str) {
    // Read the IDT entry for vector 32 (timer interrupt)
    if let Some(idt) = IDT.get() {
        let _entry = &idt[InterruptIndex::Timer.as_u8()];

        // Get the handler address from the IDT entry
        // The x86_64 crate doesn't expose this directly, so we need to read IDTR
        unsafe {
            let idtr = x86_64::instructions::tables::sidt();
            let idt_base = idtr.base.as_ptr() as *const u64;

            // Each IDT entry is 16 bytes
            let entry_offset = InterruptIndex::Timer.as_usize() * 2;
            let entry_ptr = idt_base.add(entry_offset);

            // Read the two 64-bit words that make up the IDT entry
            let low = core::ptr::read_volatile(entry_ptr);
            let high = core::ptr::read_volatile(entry_ptr.add(1));

            // Extract handler address from IDT entry format:
            // Low word: bits 0-15: offset low, bits 48-63: offset mid
            // High word: bits 0-31: offset high
            let offset_low = low & 0xFFFF;
            let offset_mid = (low >> 48) & 0xFFFF;
            let offset_high = (high & 0xFFFFFFFF) << 32;
            let handler_addr = offset_low | (offset_mid << 16) | offset_high;

            // Validate the handler address
            if handler_addr == 0 {
                return (false, 0, "Handler address is NULL");
            }

            // Check if the address looks like kernel code (should be in high half or low kernel region)
            if handler_addr < 0x100000 && handler_addr > 0x1000 {
                return (false, handler_addr, "Handler address looks invalid (in low memory)");
            }

            (true, handler_addr, "Handler address valid")
        }
    } else {
        (false, 0, "IDT not initialized")
    }
}

/// Check if interrupts are currently enabled
pub fn are_interrupts_enabled() -> bool {
    x86_64::instructions::interrupts::are_enabled()
}

/// Validate that the PIC has IRQ0 (timer) unmasked
/// Returns (is_unmasked, mask_value, description)
pub fn validate_pic_irq0_unmasked() -> (bool, u8, &'static str) {
    unsafe {
        use x86_64::instructions::port::Port;
        let mut pic1_data = Port::<u8>::new(0x21);
        let mask = pic1_data.read();

        // Bit 0 should be clear (0) for IRQ0 to be unmasked
        let irq0_masked = (mask & 0x01) != 0;

        if irq0_masked {
            (false, mask, "IRQ0 is MASKED (bit 0 set)")
        } else {
            (true, mask, "IRQ0 is UNMASKED (bit 0 clear)")
        }
    }
}
