use crate::gdt;

use pic8259::ChainedPics;
use spin::Once;
use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame, PageFaultErrorCode};
use x86_64::VirtAddr;

// Import HAL for architecture-specific operations
use crate::arch_impl::PageTableOps;
use crate::arch_impl::current::paging::X86PageTableOps;

pub(crate) mod context_switch;
mod timer;

pub const PIC_1_OFFSET: u8 = 32;
pub const PIC_2_OFFSET: u8 = PIC_1_OFFSET + 8;

pub static PICS: spin::Mutex<ChainedPics> =
    spin::Mutex::new(unsafe { ChainedPics::new(PIC_1_OFFSET, PIC_2_OFFSET) });

// NOTE: VirtIO block handler callback mechanism temporarily removed.
// The atomic static was causing boot hangs at STEP 3 (IST stack initialization).
// The VirtIO IRQ is not unmasked during boot anyway, so the handler won't be called.
// This can be re-added once the root cause is understood.

#[derive(Debug, Clone, Copy)]
#[repr(u8)]
pub enum InterruptIndex {
    Timer = PIC_1_OFFSET,
    Keyboard,
    // Skip COM2 (IRQ3)
    Serial = PIC_1_OFFSET + 4, // COM1 is IRQ4
    // IRQ 11 is shared by VirtIO block and E1000 network devices (on PIC2)
    Irq11 = PIC_2_OFFSET + 3, // IRQ 11 = 40 + 3 = 43
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
        idt[InterruptIndex::Irq11.as_u8()].set_handler_fn(irq11_handler);

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
                && i != InterruptIndex::Irq11.as_u8()
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

        // Unmask timer (IRQ0), keyboard (IRQ1), and serial (IRQ4) interrupts on PIC1
        // NOTE: Do NOT unmask IRQ 11 (VirtIO) here - it must be unmasked AFTER
        // the VirtIO driver is initialized, otherwise spurious interrupts during
        // early boot will call get_device() on uninitialized state.
        use x86_64::instructions::port::Port;
        let mut port1: Port<u8> = Port::new(0x21); // PIC1 data port
        let mask1 = port1.read() & !0b00010011; // Clear bit 0 (timer), bit 1 (keyboard), and bit 4 (serial)
        port1.write(mask1);

        // Drain any pending keyboard data to reset the controller state
        // This ensures the keyboard interrupt line is ready for new interrupts
        let mut kb_status: Port<u8> = Port::new(0x64);
        let mut kb_data: Port<u8> = Port::new(0x60);
        for _ in 0..10 {
            if (kb_status.read() & 0x01) != 0 {
                let _ = kb_data.read(); // Drain pending data
            } else {
                break;
            }
        }
    }
}

/// Enable IRQ 11 (shared by VirtIO block and E1000 network)
///
/// IMPORTANT: Only call this AFTER devices using IRQ 11 have been initialized.
/// Calling earlier will cause hangs due to interrupt handler accessing
/// uninitialized driver state.
pub fn enable_irq11() {
    unsafe {
        use x86_64::instructions::port::Port;

        // Unmask IRQ 11 (bit 3 on PIC2)
        let mut port2: Port<u8> = Port::new(0xA1); // PIC2 data port
        let mask2 = port2.read() & !0b00001000; // Clear bit 3 (IRQ 11 = 8 + 3)
        port2.write(mask2);

        // Ensure cascade (IRQ2) is unmasked on PIC1
        let mut port1: Port<u8> = Port::new(0x21); // PIC1 data port
        let mask1_cascade = port1.read() & !0b00000100; // Clear bit 2 (cascade)
        port1.write(mask1_cascade);

        log::debug!("IRQ 11 enabled (VirtIO + E1000)");
    }
}

/// Legacy alias for enable_irq11
pub fn enable_virtio_irq() {
    enable_irq11();
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
    use crate::arch_impl::current::paging;

    // DIAGNOSTIC OUTPUT AT THE VERY START
    let cr2: u64;
    let cr3: u64;
    let actual_rsp: u64;
    unsafe {
        // Use HAL for CR2/CR3 access
        cr2 = paging::read_page_fault_address().unwrap_or(0);
        cr3 = X86PageTableOps::read_root();
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
    
    // Check current page table via HAL
    log::error!("Current CR3: {:#x}", X86PageTableOps::read_root());
    
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

    // DEBUG: Print "KEY:XX " on EVERY keyboard interrupt (raw serial, no locks)
    let scancode: u8;
    unsafe {
        // Read scancode from keyboard controller
        let mut kb_port: Port<u8> = Port::new(0x60);
        scancode = kb_port.read();

        // Print to serial: "KEY:XX "
        let mut serial_port: Port<u8> = Port::new(0x3F8);
        serial_port.write(b'K');
        serial_port.write(b'E');
        serial_port.write(b'Y');
        serial_port.write(b':');
        let hi = (scancode >> 4) & 0x0F;
        let lo = scancode & 0x0F;
        serial_port.write(if hi < 10 { b'0' + hi } else { b'A' + hi - 10 });
        serial_port.write(if lo < 10 { b'0' + lo } else { b'A' + lo - 10 });
        serial_port.write(b' ');
    }

    // Enter hardware IRQ context
    crate::per_cpu::irq_enter();

    // Handle terminal switching keys (F1/F2) in interactive mode
    #[cfg(feature = "interactive")]
    {
        if crate::graphics::terminal_manager::handle_terminal_key(scancode) {
            unsafe {
                PICS.lock()
                    .notify_end_of_interrupt(InterruptIndex::Keyboard.as_u8());
            }
            crate::per_cpu::irq_exit();
            return;
        }
    }

    // Process scancode to get key event
    if let Some(event) = crate::keyboard::process_scancode(scancode) {
        if let Some(character) = event.character {
            let c = character as u8;
            // Route through TTY
            let _ = crate::tty::driver::push_char_nonblock(c);
        }
    }

    unsafe {
        PICS.lock()
            .notify_end_of_interrupt(InterruptIndex::Keyboard.as_u8());
    }

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
        // Add to serial queue for async serial console processing
        // Note: Serial input is kept separate from stdin (keyboard input)
        // This follows proper Unix design where serial and keyboard are different devices
        crate::serial::add_serial_byte(byte);
    }

    unsafe {
        PICS.lock()
            .notify_end_of_interrupt(InterruptIndex::Serial.as_u8());
    }

    // Exit hardware IRQ context
    crate::per_cpu::irq_exit();
}


/// Shared IRQ 11 handler for VirtIO block and E1000 network devices
///
/// CRITICAL: This handler must be extremely fast. No logging, no allocations.
/// Target: <1000 cycles total.
extern "x86-interrupt" fn irq11_handler(_stack_frame: InterruptStackFrame) {
    // Enter hardware IRQ context
    crate::per_cpu::irq_enter();

    // Dispatch to VirtIO block if present
    if let Some(device) = crate::drivers::virtio::block::get_device() {
        device.handle_interrupt();
    }

    // Dispatch to E1000 network if initialized
    crate::drivers::e1000::handle_interrupt();

    // Send EOI to both PICs (IRQ 11 is on PIC2)
    unsafe {
        PICS.lock()
            .notify_end_of_interrupt(InterruptIndex::Irq11.as_u8());
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
        use crate::arch_impl::CpuOps;
        crate::arch_impl::current::cpu::X86Cpu::halt();
    }
    
    // Note: preempt_enable() not called here since we enter infinite loop or exit
}

/// Handle a Copy-on-Write page fault
///
/// This function is called when a page fault occurs due to a write to a
/// CoW-shared page. It:
/// 1. Checks if the page has the COW_FLAG set
/// 2. If the frame is no longer shared (refcount == 1), just makes it writable
/// 3. Otherwise, allocates a new frame, copies the page, and remaps
///
/// Returns true if the fault was handled (was a CoW fault), false otherwise.
///
/// IMPORTANT: This function uses try_manager() to avoid deadlock when called
/// during signal delivery (which holds the process manager lock). If the lock
/// is held, we handle the CoW fault directly by manipulating page tables via CR3.
/// Copy-on-Write statistics for testing and debugging
pub mod cow_stats {
    use core::sync::atomic::{AtomicU64, Ordering};

    /// Total CoW faults handled
    pub static TOTAL_FAULTS: AtomicU64 = AtomicU64::new(0);
    /// Faults handled via process manager (normal path)
    pub static MANAGER_PATH: AtomicU64 = AtomicU64::new(0);
    /// Faults handled via direct page table manipulation (lock-held path)
    pub static DIRECT_PATH: AtomicU64 = AtomicU64::new(0);
    /// Pages that were copied (frame was shared)
    pub static PAGES_COPIED: AtomicU64 = AtomicU64::new(0);
    /// Pages made writable without copy (sole owner optimization)
    pub static SOLE_OWNER_OPT: AtomicU64 = AtomicU64::new(0);

    /// Get current CoW statistics
    #[allow(dead_code)]
    pub fn get_stats() -> CowStats {
        CowStats {
            total_faults: TOTAL_FAULTS.load(Ordering::Relaxed),
            manager_path: MANAGER_PATH.load(Ordering::Relaxed),
            direct_path: DIRECT_PATH.load(Ordering::Relaxed),
            pages_copied: PAGES_COPIED.load(Ordering::Relaxed),
            sole_owner_opt: SOLE_OWNER_OPT.load(Ordering::Relaxed),
        }
    }

    /// Reset all statistics (for testing)
    #[allow(dead_code)]
    pub fn reset_stats() {
        TOTAL_FAULTS.store(0, Ordering::Relaxed);
        MANAGER_PATH.store(0, Ordering::Relaxed);
        DIRECT_PATH.store(0, Ordering::Relaxed);
        PAGES_COPIED.store(0, Ordering::Relaxed);
        SOLE_OWNER_OPT.store(0, Ordering::Relaxed);
    }

    /// CoW statistics snapshot
    #[allow(dead_code)]
    #[derive(Debug, Clone, Copy)]
    pub struct CowStats {
        pub total_faults: u64,
        pub manager_path: u64,
        pub direct_path: u64,
        pub pages_copied: u64,
        pub sole_owner_opt: u64,
    }

    #[allow(dead_code)]
    impl CowStats {
        /// Print statistics to serial output
        pub fn print(&self) {
            crate::serial_println!(
                "[COW STATS] total={} manager={} direct={} copied={} sole_owner={}",
                self.total_faults,
                self.manager_path,
                self.direct_path,
                self.pages_copied,
                self.sole_owner_opt
            );
        }
    }
}

fn handle_cow_fault(
    faulting_addr: VirtAddr,
    error_code: PageFaultErrorCode,
    cr3: u64,
) -> bool {
    // CoW faults are:
    // - Protection violation (page is present but not writable)
    // - Caused by write
    if !error_code.contains(PageFaultErrorCode::PROTECTION_VIOLATION) {
        return false;
    }
    if !error_code.contains(PageFaultErrorCode::CAUSED_BY_WRITE) {
        return false;
    }

    // Track CoW fault count for debugging
    let fault_num = cow_stats::TOTAL_FAULTS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    if fault_num < 20 {
        crate::serial_println!(
            "[COW FAULT #{}] addr={:#x} cr3={:#x}",
            fault_num,
            faulting_addr.as_u64(),
            cr3
        );
    }

    // Try to acquire process manager lock. If it's held (e.g., by signal delivery),
    // we'll handle the CoW fault directly via CR3 to avoid deadlock.
    match crate::process::try_manager() {
        Some(mut guard) => {
            // Lock acquired, proceed with normal CoW handling via ProcessPageTable
            cow_stats::MANAGER_PATH.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            handle_cow_with_manager(&mut guard, faulting_addr, cr3)
        }
        None => {
            // Lock is held - handle CoW directly via CR3 to avoid deadlock
            // This can happen during signal delivery which writes to user stack
            cow_stats::DIRECT_PATH.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            if fault_num < 20 {
                crate::serial_println!("[COW FAULT #{}] lock held, using direct path", fault_num);
            }
            handle_cow_direct(faulting_addr, cr3)
        }
    }
}

/// Handle CoW fault through the process manager (normal path)
fn handle_cow_with_manager(
    guard: &mut spin::MutexGuard<'static, Option<crate::process::ProcessManager>>,
    faulting_addr: VirtAddr,
    cr3: u64,
) -> bool {
    use crate::memory::frame_allocator::allocate_frame;
    use crate::memory::frame_metadata::{frame_decref, frame_is_shared};
    use crate::memory::process_memory::{is_cow_page, make_private_flags};
    use x86_64::structures::paging::{Page, Size4KiB};

    let pm = match guard.as_mut() {
        Some(pm) => pm,
        None => return false,
    };

    let (_pid, process) = match pm.find_process_by_cr3_mut(cr3) {
        Some(p) => p,
        None => return false,
    };

    let page_table = match &mut process.page_table {
        Some(pt) => pt,
        None => return false,
    };

    let page = Page::<Size4KiB>::containing_address(faulting_addr);

    // Get the current page info
    let (old_frame, old_flags) = match page_table.get_page_info(page) {
        Some(info) => info,
        None => return false,
    };

    // Check if this is actually a CoW page
    if !is_cow_page(old_flags) {
        return false;
    }

    // Check if we're the only reference - can just make it writable
    if !frame_is_shared(old_frame) {
        let new_flags = make_private_flags(old_flags);
        if page_table.update_page_flags(page, new_flags).is_err() {
            return false;
        }
        X86PageTableOps::flush_tlb_page(faulting_addr.as_u64());
        cow_stats::SOLE_OWNER_OPT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        return true;
    }

    // Multiple references - need to copy
    let new_frame = match allocate_frame() {
        Some(frame) => frame,
        None => return false,
    };

    // Copy page contents
    let phys_offset = crate::memory::physical_memory_offset();
    unsafe {
        let src = (phys_offset + old_frame.start_address().as_u64()).as_ptr::<u8>();
        let dst = (phys_offset + new_frame.start_address().as_u64()).as_mut_ptr::<u8>();
        core::ptr::copy_nonoverlapping(src, dst, 4096);
    }

    // Update page table: unmap old, map new with writable flags
    let new_flags = make_private_flags(old_flags);
    if page_table.unmap_page(page).is_err() {
        return false;
    }
    if page_table.map_page(page, new_frame, new_flags).is_err() {
        return false;
    }

    // Decrement old frame reference count
    frame_decref(old_frame);

    // Flush TLB for this page
    X86PageTableOps::flush_tlb_page(faulting_addr.as_u64());
    cow_stats::PAGES_COPIED.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    true
}

/// Handle CoW fault directly via CR3 (used when process manager lock is held)
///
/// This function walks the page table manually and modifies entries directly,
/// avoiding the need to acquire the process manager lock.
fn handle_cow_direct(faulting_addr: VirtAddr, cr3: u64) -> bool {
    use crate::memory::frame_allocator::allocate_frame;
    use crate::memory::frame_metadata::{frame_decref, frame_is_shared};
    use crate::memory::process_memory::{is_cow_page, make_private_flags};
    use x86_64::structures::paging::{PageTable, PageTableFlags, PhysFrame, Size4KiB};

    let phys_offset = crate::memory::physical_memory_offset();
    let virt_addr = faulting_addr;

    // Walk the page table hierarchy to find the L1 entry
    unsafe {
        // L4 table
        let l4_virt = phys_offset + cr3;
        let l4_table = &mut *(l4_virt.as_mut_ptr() as *mut PageTable);
        let l4_idx = ((virt_addr.as_u64() >> 39) & 0x1FF) as usize;
        let l4_entry = &l4_table[l4_idx];

        if l4_entry.is_unused() || !l4_entry.flags().contains(PageTableFlags::PRESENT) {
            return false;
        }

        // L3 table
        let l3_virt = phys_offset + l4_entry.addr().as_u64();
        let l3_table = &mut *(l3_virt.as_mut_ptr() as *mut PageTable);
        let l3_idx = ((virt_addr.as_u64() >> 30) & 0x1FF) as usize;
        let l3_entry = &l3_table[l3_idx];

        if l3_entry.is_unused() || !l3_entry.flags().contains(PageTableFlags::PRESENT) {
            return false;
        }
        if l3_entry.flags().contains(PageTableFlags::HUGE_PAGE) {
            return false; // 1GB huge pages not supported for CoW
        }

        // L2 table
        let l2_virt = phys_offset + l3_entry.addr().as_u64();
        let l2_table = &mut *(l2_virt.as_mut_ptr() as *mut PageTable);
        let l2_idx = ((virt_addr.as_u64() >> 21) & 0x1FF) as usize;
        let l2_entry = &l2_table[l2_idx];

        if l2_entry.is_unused() || !l2_entry.flags().contains(PageTableFlags::PRESENT) {
            return false;
        }
        if l2_entry.flags().contains(PageTableFlags::HUGE_PAGE) {
            return false; // 2MB huge pages not supported for CoW
        }

        // L1 table - this is where we modify the entry
        let l1_virt = phys_offset + l2_entry.addr().as_u64();
        let l1_table = &mut *(l1_virt.as_mut_ptr() as *mut PageTable);
        let l1_idx = ((virt_addr.as_u64() >> 12) & 0x1FF) as usize;
        let l1_entry = &mut l1_table[l1_idx];

        if l1_entry.is_unused() || !l1_entry.flags().contains(PageTableFlags::PRESENT) {
            return false;
        }

        let old_flags = l1_entry.flags();
        let old_frame = PhysFrame::<Size4KiB>::containing_address(l1_entry.addr());

        // Check if this is a CoW page
        if !is_cow_page(old_flags) {
            return false;
        }

        // Check if we're the only reference
        if !frame_is_shared(old_frame) {
            // Sole owner - just update flags to make writable
            let new_flags = make_private_flags(old_flags);
            l1_entry.set_addr(l1_entry.addr(), new_flags);
            X86PageTableOps::flush_tlb_page(faulting_addr.as_u64());
            cow_stats::SOLE_OWNER_OPT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            return true;
        }

        // Multiple references - need to copy
        let new_frame = match allocate_frame() {
            Some(frame) => frame,
            None => return false,
        };

        // Copy page contents
        let src = (phys_offset + old_frame.start_address().as_u64()).as_ptr::<u8>();
        let dst = (phys_offset + new_frame.start_address().as_u64()).as_mut_ptr::<u8>();
        core::ptr::copy_nonoverlapping(src, dst, 4096);

        // Update the L1 entry with new frame and writable flags
        let new_flags = make_private_flags(old_flags);
        l1_entry.set_addr(new_frame.start_address(), new_flags);

        // Decrement old frame reference count
        frame_decref(old_frame);

        // Flush TLB
        X86PageTableOps::flush_tlb_page(faulting_addr.as_u64());
        cow_stats::PAGES_COPIED.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        true
    }
}

extern "x86-interrupt" fn page_fault_handler(
    mut stack_frame: InterruptStackFrame,
    error_code: PageFaultErrorCode,
) {
    use x86_64::registers::control::Cr2;

    // Read CR2 and CR3 first
    let cr2 = Cr2::read().unwrap_or(x86_64::VirtAddr::zero()).as_u64();
    let cr3 = {
        use x86_64::registers::control::Cr3;
        let (frame, _) = Cr3::read();
        frame.start_address().as_u64()
    };

    // Check if this looks like a CoW fault (protection violation + write)
    // If so, skip verbose diagnostics to avoid polluting output and slowing down
    let is_potential_cow = error_code.contains(PageFaultErrorCode::PROTECTION_VIOLATION)
        && error_code.contains(PageFaultErrorCode::CAUSED_BY_WRITE);

    // Only print verbose diagnostics for non-CoW faults
    if !is_potential_cow {
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
    }

    // Increment preempt count on exception entry FIRST to avoid recursion
    crate::per_cpu::preempt_disable();

    // Use the cr2 value we already read safely above (line 894)
    let accessed_addr = x86_64::VirtAddr::new(cr2);

    // Skip raw serial output for potential CoW faults
    if !is_potential_cow {
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
    }

    // Only print verbose diagnostics for non-CoW faults
    if !is_potential_cow {
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
    }
    
    // Quick debug output for int3 test - only for non-CoW faults
    if !is_potential_cow {
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

    // Try to handle as Copy-on-Write fault
    // This handles writes to pages that were marked read-only during fork()
    // We check if the address is in userspace (< 0x8000_0000_0000) rather than
    // just checking if the fault came from userspace. This allows the kernel
    // to trigger CoW when writing to user memory (e.g., signal frame setup).
    let is_user_address = accessed_addr.as_u64() < crate::memory::layout::USER_STACK_REGION_END;
    if is_user_address && handle_cow_fault(accessed_addr, error_code, cr3) {
        // CoW fault handled successfully - resume execution
        crate::per_cpu::preempt_enable();
        return;
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

            // CRITICAL: Set exception cleanup context so can_schedule() returns true
            // This allows scheduling from kernel mode after terminating a process
            crate::per_cpu::set_exception_cleanup_context();

            // CRITICAL: Update scheduler to point to idle thread BEFORE modifying exception frame.
            // This ensures subsequent timer interrupts can properly schedule other threads.
            crate::task::scheduler::switch_to_idle();

            // CRITICAL FIX: Instead of entering an hlt loop (which doesn't work because
            // timer interrupts can't properly schedule from exception context), modify
            // the exception frame to return directly to the idle loop.
            //
            // NOTE: CR3 was already switched to kernel page table above. DO NOT call
            // switch_to_kernel_page_table() again - redundant CR3 writes with TLB flush
            // can cause hangs when on the IST stack.
            unsafe {
                stack_frame.as_mut().update(|frame| {
                    frame.code_segment = crate::gdt::kernel_code_selector();
                    frame.stack_segment = crate::gdt::kernel_data_selector();
                    frame.instruction_pointer = x86_64::VirtAddr::new(
                        context_switch::idle_loop as *const () as u64
                    );
                    // CRITICAL: Set both INTERRUPT_FLAG (bit 9) AND reserved bit 1 (always required)
                    // 0x202 = INTERRUPT_FLAG (0x200) | reserved bit 1 (0x002)
                    let flags_ptr = &mut frame.cpu_flags as *mut x86_64::registers::rflags::RFlags as *mut u64;
                    *flags_ptr = 0x202;

                    // CRITICAL: Use the idle thread's actual kernel stack, NOT the IST stack!
                    // The page fault handler runs on IST[1] which is small and not meant
                    // for general execution. Using current_rsp would continue on IST stack
                    // which can overflow when timer interrupts fire.
                    let idle_stack = crate::per_cpu::kernel_stack_top();
                    frame.stack_pointer = x86_64::VirtAddr::new(idle_stack);
                });
            }

            log::info!("Page fault handler: Modified exception frame to return to idle loop");

            // Return from handler - IRET will jump to idle_loop
            return;
        }

        // Kernel page fault - this is a bug, panic
        panic!("Kernel page fault at {:#x} (error: {:?})", accessed_addr.as_u64(), error_code);
    }
}

extern "x86-interrupt" fn generic_handler(stack_frame: InterruptStackFrame) {
    // Enter hardware IRQ context for unknown interrupts
    crate::per_cpu::irq_enter();

    log::warn!(
        "UNHANDLED INTERRUPT from RIP {:#x}",
        stack_frame.instruction_pointer.as_u64()
    );
    log::warn!("{:#?}", stack_frame);

    // CRITICAL: Send EOI to PICs for any hardware interrupt
    // Without this, the PIC will hang and not deliver more interrupts.
    // We send EOI to both PICs (PIC2 cascades through PIC1) to be safe.
    // This handles any interrupt vector in the range 32-47 (PIC hardware IRQs).
    unsafe {
        // Send EOI to PIC2 first (if it was a PIC2 interrupt), then PIC1
        // notify_end_of_interrupt handles this automatically based on vector
        // Use a high vector to ensure both PICs get EOI
        PICS.lock().notify_end_of_interrupt(PIC_2_OFFSET + 7);
    }

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
    mut stack_frame: InterruptStackFrame,
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

        // CRITICAL: Set exception cleanup context so can_schedule() returns true
        // This allows scheduling from kernel mode after terminating a process
        crate::per_cpu::set_exception_cleanup_context();

        // CRITICAL: Update scheduler to point to idle thread BEFORE modifying exception frame.
        // This ensures subsequent timer interrupts can properly schedule other threads.
        crate::task::scheduler::switch_to_idle();

        // CRITICAL FIX: Instead of entering an hlt loop (which doesn't work because
        // timer interrupts can't properly schedule from exception context), modify
        // the exception frame to return directly to the idle loop.
        //
        // NOTE: CR3 was already switched to kernel page table above. DO NOT call
        // switch_to_kernel_page_table() again - redundant CR3 writes with TLB flush
        // can cause hangs when on the IST stack.
        unsafe {
            stack_frame.as_mut().update(|frame| {
                frame.code_segment = crate::gdt::kernel_code_selector();
                frame.stack_segment = crate::gdt::kernel_data_selector();
                frame.instruction_pointer = x86_64::VirtAddr::new(
                    context_switch::idle_loop as *const () as u64
                );
                // CRITICAL: Set both INTERRUPT_FLAG (bit 9) AND reserved bit 1 (always required)
                // 0x202 = INTERRUPT_FLAG (0x200) | reserved bit 1 (0x002)
                let flags_ptr = &mut frame.cpu_flags as *mut x86_64::registers::rflags::RFlags as *mut u64;
                *flags_ptr = 0x202;

                // Set up kernel stack - use current RSP with some headroom
                let current_rsp: u64;
                core::arch::asm!("mov {}, rsp", out(reg) current_rsp);
                frame.stack_pointer = x86_64::VirtAddr::new(current_rsp + 256);
            });
        }

        log::info!("GPF handler: Modified exception frame to return to idle loop");

        // Return from handler - IRET will jump to idle_loop
        return;
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
