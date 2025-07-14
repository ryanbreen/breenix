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
    fn timer_interrupt_entry();
}

impl InterruptIndex {
    pub fn as_u8(self) -> u8 {
        self as u8
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
        idt.breakpoint.set_handler_fn(breakpoint_handler)
            .set_privilege_level(x86_64::PrivilegeLevel::Ring3);
        
        // Debug exception handler for single-step
        #[cfg(feature = "testing")]
        idt.debug.set_handler_fn(debug_exception);
        #[cfg(not(feature = "testing"))]
        idt.debug.set_handler_fn(debug_exception_handler);
        
        idt.invalid_opcode.set_handler_fn(invalid_opcode_handler);
        // Add minimal #GP, #SS, and #NP handlers to detect IRETQ validation failures
        extern "C" {
            fn gp_stub();
            fn ss_stub();
            fn np_stub();
        }
        unsafe {
            idt.general_protection_fault.set_handler_addr(VirtAddr::new(gp_stub as u64));
            idt.stack_segment_fault.set_handler_addr(VirtAddr::new(ss_stub as u64));
            idt.segment_not_present.set_handler_addr(VirtAddr::new(np_stub as u64));
        }
        // Install double fault handler with IST[1] for Ring 3 timer debugging  
        unsafe {
            extern "C" {
                fn df_stub();
            }
            idt.double_fault.set_handler_addr(VirtAddr::new(df_stub as u64))
                .set_stack_index(crate::gdt::DEBUG_DF_IST_INDEX);
        }
        idt.page_fault.set_handler_fn(page_fault_handler);
        
        // Hardware interrupt handlers
        // Timer interrupt with proper interrupt return path handling
        unsafe {
            let timer_addr = timer_interrupt_entry as u64;
            idt[InterruptIndex::Timer.as_u8()].set_handler_addr(VirtAddr::new(timer_addr));
            log::info!("TIMER_VECTOR points to: {:#x}", timer_addr);
            
            // Also check what's actually in the IDT after loading
            if let Some(idt_ptr) = IDT.get() {
                let actual_addr = idt_ptr[InterruptIndex::Timer.as_u8()].handler_addr();
                log::info!("IDT[timer] actually contains: {:#x}", actual_addr.as_u64());
            } else {
                log::info!("IDT not yet initialized, can't check timer vector");
            }
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
    
    // Dump IDT[0x80] raw gate configuration for debugging
    unsafe {
        let idt_entry_ptr = (idt_ptr + (SYSCALL_INTERRUPT_ID as u64 * 16)) as *const u64;
        let low_word = *idt_entry_ptr;
        let high_word = *idt_entry_ptr.add(1);
        crate::serial_println!("IDT80 raw={:#016x}{:016x}", high_word, low_word);
        
        // Extract and log DPL from the gate (bits 45-46 of the 128-bit entry)
        // Bits 45-46 are in the low part of the high word (high word starts at bit 64)
        let dpl = ((low_word >> 45) & 0x3) as u8;
        crate::serial_println!("IDT[0x80] DPL={} (should be 3 for user access)", dpl);
    }
    
    // CRITICAL CHECK: Verify IDT[1] debug gate configuration
    #[cfg(feature = "testing")]
    unsafe {
        let debug_entry_ptr = (idt_ptr + (1 * 16)) as *const u64;
        let debug_low = *debug_entry_ptr;
        let debug_high = *debug_entry_ptr.add(1);
        crate::serial_println!("IDT[1] debug gate raw={:#016x}{:016x}", debug_high, debug_low);
        
        // Check if present bit is set (bit 47)
        let present = (debug_low >> 47) & 1;
        crate::serial_println!("IDT[1] PRESENT={} (should be 1)", present);
    }
}

/// Called from assembly to print IRET stack dump
#[no_mangle]
pub extern "C" fn serial_print_iret_stack(stack_ptr: *const u64) {
    unsafe {
        let rip = *stack_ptr;
        let cs = *stack_ptr.add(1);
        let rflags = *stack_ptr.add(2);
        let rsp = *stack_ptr.add(3);
        let ss = *stack_ptr.add(4);
        
        crate::serial_println!(
            "IRET STACK: RIP={:#x} CS={:#x} RFLAGS={:#x} RSP={:#x} SS={:#x}",
            rip, cs, rflags, rsp, ss
        );
    }
}

/// Called from assembly after CR3 switch
#[no_mangle]
pub extern "C" fn serial_println_from_asm_cr3_done() {
    crate::serial_println!("ASM_CR3_DONE: Successfully completed CR3 switch in assembly");
}

/// Called from assembly right before iretq
#[no_mangle]
pub extern "C" fn serial_println_from_asm_before_iretq() {
    crate::serial_println!("ASM_BEFORE_IRETQ: About to execute iretq to userspace");
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

/// Debug function to temporarily disable timer interrupts
#[allow(dead_code)]
pub fn disable_timer_interrupt() {
    unsafe {
        use x86_64::instructions::port::Port;
        let mut port: Port<u8> = Port::new(0x21); // PIC1 data port
        let mask = port.read() | 0b00000001; // Set bit 0 (timer) to mask it
        port.write(mask);
        log::warn!("DEBUG: Timer interrupt disabled");
    }
}

/// Debug function to re-enable timer interrupts
#[allow(dead_code)]
pub fn enable_timer_interrupt() {
    unsafe {
        use x86_64::instructions::port::Port;
        let mut port: Port<u8> = Port::new(0x21); // PIC1 data port
        let mask = port.read() & !0b00000001; // Clear bit 0 (timer) to unmask it
        port.write(mask);
        log::warn!("DEBUG: Timer interrupt enabled");
    }
}

#[cfg(feature = "testing")]
extern "x86-interrupt" fn debug_exception(
    mut stack: x86_64::structures::idt::InterruptStackFrame
) {
    // CRITICAL: Unconditional print to verify handler entry
    crate::serial_println!("### DB ENTER cs={:#x} rip={:#x}", stack.code_segment.0, stack.instruction_pointer.as_u64());
    
    let rip = stack.instruction_pointer.as_u64();
    let cs  = stack.code_segment.0;
    crate::serial_println!("STEP_1: cs={:#x} rip={:#x}", cs, rip);

    // clear TF so execution continues at full speed
    unsafe {
        stack.as_mut().update(|frame| {
            let mut flags = frame.cpu_flags;
            flags.remove(x86_64::registers::rflags::RFlags::TRAP_FLAG);
            frame.cpu_flags = flags;
        });
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
        
        // Get current thread ID to check if it's the child
        if let Some(thread_id) = crate::task::scheduler::current_thread_id() {
            log::warn!("INT3 HIT: Thread {} executed instruction at {:#x}!", 
                     thread_id, stack_frame.instruction_pointer.as_u64());
        }
        
        // Action 4.1: Dump RFLAGS in breakpoint handler
        let rflags: u64;
        unsafe { 
            core::arch::asm!("pushfq; pop {}", out(reg) rflags); 
        }
        crate::serial_println!("RFLAGS user = {:#x}", rflags);
        
        // Action 3.1: Spin-in-place test to verify PIC/APIC fires after CR3 switch
        crate::serial_println!("SPIN_TEST: Starting 20ms spin with interrupts enabled...");
        use x86_64::instructions::interrupts;
        
        // Enable interrupts and spin for a bit to let timer IRQ fire
        for i in 0..5 {
            crate::serial_println!("SPIN_TEST: Loop {} - enabling interrupts", i);
            interrupts::enable_and_hlt();   // Let *any* IRQ in
            
            // Small delay
            for _ in 0..100000 { 
                core::hint::spin_loop(); 
            }
        }
        crate::serial_println!("SPIN_TEST: Completed - check for timer IRQ patterns");
    } else {
        log::info!("EXCEPTION: BREAKPOINT\n{:#?}", stack_frame);
    }
}

extern "x86-interrupt" fn debug_exception_handler(mut stack_frame: InterruptStackFrame) {
    // NO-CRATE TF LITMUS: Debug handler for single-step trap flag
    let rip = stack_frame.instruction_pointer.as_u64();
    let cs = stack_frame.code_segment.0;
    
    crate::serial_println!("#DB rip={:#x} cs={:#x}", rip, cs);
    
    // 3-E: Userspace single-step tracing
    #[cfg(feature = "testing")]
    {
        static mut STEP_COUNT: u32 = 0;
        const MAX_STEPS: u32 = 10; // Trace first 10 instructions
        
        unsafe {
            if (cs & 3) == 3 { // Userspace
                STEP_COUNT += 1;
                crate::serial_println!("STEP_{}: USERSPACE RIP={:#x}", STEP_COUNT, rip);
                
                if STEP_COUNT < MAX_STEPS {
                    // Keep TF set to continue single-stepping
                    stack_frame.as_mut().update(|frame| {
                        let flags_bits = frame.cpu_flags.bits();
                        frame.cpu_flags = x86_64::registers::rflags::RFlags::from_bits_truncate(flags_bits | 0x100);
                    });
                    return; // Continue single-stepping
                } else {
                    crate::serial_println!("STEP_COMPLETE: Traced {} userspace instructions", STEP_COUNT);
                    STEP_COUNT = 0; // Reset for next process
                }
            }
        }
    }
    
    // Clear TF bit in the saved RFLAGS on the interrupt stack frame (normal path)
    unsafe {
        stack_frame.as_mut().update(|frame| {
            let flags_bits = frame.cpu_flags.bits();
            frame.cpu_flags = x86_64::registers::rflags::RFlags::from_bits_truncate(flags_bits & !0x100);
        });
    }
    
    // Interpret the result based on CS
    if (cs & 3) == 3 {
        crate::serial_println!("SUCCESS: IRETQ executed! Userspace at {:#x}", rip);
    } else {
        crate::serial_println!("DEBUG: Kernel debug at {:#x}", rip);
    }
}

// Emergency double fault handler removed due to signature compatibility issues

extern "x86-interrupt" fn double_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: u64,
) {
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
    
    // This should never be reached but ensures the function diverges
    #[allow(unreachable_code)]
    loop {
        x86_64::instructions::hlt();
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
    
    // Check if we're in test mode
    if crate::test_harness::is_test_mode() {
        log::warn!("TEST_MARKER: DIV0_OK");
        // For testing, we'll exit cleanly instead of panicking
        crate::test_exit_qemu(crate::QemuExitCode::Success);
    } else {
        panic!("Kernel halted due to divide by zero exception");
    }
}

extern "x86-interrupt" fn invalid_opcode_handler(stack_frame: InterruptStackFrame) {
    log::error!("EXCEPTION: INVALID OPCODE at {:#x}\n{:#?}", 
        stack_frame.instruction_pointer.as_u64(), stack_frame);
    
    // Check if we're in test mode
    if crate::test_harness::is_test_mode() {
        log::warn!("TEST_MARKER: INVALID_OPCODE_HANDLED");
        crate::test_exit_qemu(crate::QemuExitCode::Success);
    } else {
        loop {
            x86_64::instructions::hlt();
        }
    }
}

// External assembly debug variable
extern "C" {
    static _debug_seen_cr3: u64;
}

extern "x86-interrupt" fn page_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: PageFaultErrorCode,
) {
    // 3-A: PF exception breadcrumb
    #[cfg(feature = "testing")]
    {
        let rip = stack_frame.instruction_pointer.as_u64();
        let cs = stack_frame.code_segment.0;
        crate::serial_println!("#EXC type=PF cs={:#x} rip={:#x}", cs, rip);
    }
    
    use x86_64::registers::control::Cr2;
    
    let accessed_addr = Cr2::read().expect("Failed to read accessed address from CR2");
    let fault_addr = accessed_addr.as_u64();
    let rip = stack_frame.instruction_pointer.as_u64();
    
    // Low page fault detection - check for bad pointers  
    if fault_addr < 0x1000 {
        crate::serial_println!("LOW PF: addr={:#x} rip={:#x}", fault_addr, rip);
        crate::serial_println!("LOW PF: This is likely a bad pointer or uninitialized variable");
    }
    
    // DEBUG: Log what CR3 value the assembly code saw
    unsafe {
        log::error!("DEBUG SEEN_CR3 = {:#x}", _debug_seen_cr3);
    }
    
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
    
    // TEST 2.2: Enhanced page fault debugging
    let rip = stack_frame.instruction_pointer.as_u64();
    let cs = stack_frame.code_segment.0;
    
    crate::serial_println!("PF @ addr={:#x} rip={:#x} cs={:#x}", accessed_addr.as_u64(), rip, cs);
    
    // Check if this is a kernel page fault - enhanced debugging for heap faults
    if cs & 0x3 == 0 { // Ring 0 (kernel)
        let fault_addr = accessed_addr.as_u64();
        
        // Special handling for heap faults (0x444444440000 - 0x44444453f000)
        if fault_addr >= 0x444444440000 && fault_addr < 0x44444453f000 {
            crate::serial_println!("KHEAP PF  addr={:#x} rip={:#x}", fault_addr, rip);
            
            // Enhanced stack trace using RBP chain
            unsafe {
                let mut rbp: u64;
                core::arch::asm!("mov {}, rbp", out(reg) rbp);
                
                // Log initial RBP for debugging
                crate::serial_println!("BACKTRACE: Initial RBP={:#x}", rbp);
                
                // Dump full backtrace with better bounds checking
                for i in 0..12 {
                    // Check for valid frame pointer
                    if rbp < 0x1000_000 || rbp > 0xFFFF_FFFF_FFFF_0000 || rbp == 0 {
                        crate::serial_println!("BT[{}] = END (invalid RBP={:#x})", i, rbp);
                        break;
                    }
                    
                    // Try to read return address (RBP + 8)
                    let ret_addr_ptr = (rbp + 8) as *const u64;
                    if ret_addr_ptr as u64 > 0xFFFF_FFFF_FFFF_0000 {
                        crate::serial_println!("BT[{}] = END (invalid ret_addr_ptr={:#x})", i, ret_addr_ptr as u64);
                        break;
                    }
                    
                    let ret_addr = *(ret_addr_ptr);
                    crate::serial_println!("BT[{}] = {:#x} (RBP={:#x})", i, ret_addr, rbp);
                    
                    // Get next frame pointer
                    let next_rbp = *(rbp as *const u64);
                    
                    // Detect cycles
                    if next_rbp == rbp {
                        crate::serial_println!("BT[{}] = END (cycle detected)", i + 1);
                        break;
                    }
                    
                    rbp = next_rbp;
                }
                
                crate::serial_println!("BACKTRACE: Use 'objdump -d target/x86_64-breenix/debug/kernel | less +/ADDR' to resolve");
            }
            
            // Halt for debugging instead of panic
            crate::serial_println!("Halting for heap fault debugging...");
            loop { x86_64::instructions::hlt(); }
        } else {
            panic!("KERNEL PF at {:#x} RIP={:#x}", fault_addr, rip);
        }
    }
    
    log::error!("EXCEPTION: PAGE FAULT");
    log::error!("Accessed Address: {:?}", accessed_addr);
    log::error!("Error Code: {:?}", error_code);
    log::error!("RIP: {:#x}", stack_frame.instruction_pointer.as_u64());
    log::error!("CS: {:#x}", stack_frame.code_segment.0);
    log::error!("{:#?}", stack_frame);
    
    // Check if we're in test mode
    if crate::test_harness::is_test_mode() {
        log::warn!("TEST_MARKER: PAGE_FAULT_HANDLED");
        crate::test_exit_qemu(crate::QemuExitCode::Success);
    }
    
    #[cfg(feature = "testing")]
    {
        log::info!("TEST_MARKER: PAGE_FAULT_HANDLED");
        crate::test_exit_qemu(crate::QemuExitCode::Success);
    }
    #[cfg(not(feature = "testing"))]
    loop {
        x86_64::instructions::hlt();
    }
}

extern "x86-interrupt" fn generic_handler(stack_frame: InterruptStackFrame) {
    // 3-B: IDT vector trace - log all unhandled interrupts
    #[cfg(feature = "testing")]
    {
        let rip = stack_frame.instruction_pointer.as_u64();
        let cs = stack_frame.code_segment.0;
        crate::serial_println!("INT_VECTOR: unknown cs={:#x} rip={:#x}", cs, rip);
    }
    
    // Get the interrupt number from the stack
    // Note: This is a bit hacky but helps with debugging
    let _interrupt_num = {
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
    // 3-A: GP exception breadcrumb
    #[cfg(feature = "testing")]
    {
        let rip = stack_frame.instruction_pointer.as_u64();
        let cs = stack_frame.code_segment.0;
        crate::serial_println!("#EXC type=GP cs={:#x} rip={:#x}", cs, rip);
    }
    
    // NO-CRATE GP HANDLER: Catch immediate faults  
    let rip = stack_frame.instruction_pointer.as_u64();
    let cs = stack_frame.code_segment.0;
    
    crate::serial_println!("GP({:#x}) @ {:x}:{:x}", error_code, cs, rip);
    panic!("GP({:#x}) @ {:x}:{:x}", error_code, cs, rip);
}