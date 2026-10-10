#![cfg(target_arch = "x86_64")]

use crate::gdt;

use pic8259::ChainedPics;
use spin::Once;
use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame, PageFaultErrorCode};
use x86_64::VirtAddr;

// Import HAL for architecture-specific operations
use crate::arch_impl::current::paging::X86PageTableOps;
use crate::arch_impl::PageTableOps;

pub(crate) mod context_switch;
pub mod timer;

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
    // IRQ 10 is used by E1000 network and some VirtIO devices (on PIC2)
    Irq10 = PIC_2_OFFSET + 2, // IRQ 10 = 40 + 2 = 42
    // IRQ 11 is used by some VirtIO block devices (on PIC2)
    Irq11 = PIC_2_OFFSET + 3, // IRQ 11 = 40 + 3 = 43
}

/// System call interrupt vector (INT 0x80)
pub const SYSCALL_INTERRUPT_ID: u8 = 0x80;

/// Reschedule IPI vector. Its gate is the timer entry, so a reschedule IPI
/// returns through `check_need_resched_and_switch`, the interrupt-return path
/// that performs context switches, exactly as a tick does. The tick handler it
/// also runs charges this CPU's quantum by elapsed ticks and reads the tick
/// count from the TSC, so an extra entry changes neither.
pub const RESCHEDULE_VECTOR: u8 = 0xf0;

/// Self-IPI vector for retrying an interrupt-return step deferred because the
/// process manager was held (`scheduler::retry_after_interrupts_x86`). It is
/// the timer's own vector, 0x20, the lowest that can be delivered: every
/// device vector, the keyboard's 0x21 included, ranks above it and is taken
/// first, and it is taken as soon as the last of them returns, before the
/// next instruction of the code they return to. An extra timer entry changes
/// no accounting (see `RESCHEDULE_VECTOR`), and a self-IPI that coincides
/// with a tick is one entry that does both.
pub const RETRY_VECTOR: u8 = InterruptIndex::Timer as u8;

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

    #[allow(dead_code)] // Part of public API
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
        // x87 and SSE floating-point exceptions (CR0.NE and CR4.OSXMMEXCPT
        // are set, so they arrive here rather than as IRQ 13 or #UD) and
        // alignment checks: from Ring 3, SIGFPE and SIGBUS.
        idt.x87_floating_point.set_handler_fn(x87_floating_point_handler);
        idt.simd_floating_point.set_handler_fn(simd_floating_point_handler);
        idt.alignment_check.set_handler_fn(alignment_check_handler);
        unsafe {
            idt.double_fault
                .set_handler_fn(double_fault_handler)
                .set_stack_index(gdt::DOUBLE_FAULT_IST_INDEX);
        }
        // #PF runs on the stack the CPU is already using, as #GP does: a user
        // fault arrives on the thread's kernel stack (TSS.RSP0), and the kill
        // path below ends the process there exactly as #GP's does. It used to
        // run on an IST stack. That stack was 8 KiB with the double-fault stack
        // directly below it and no guard page between, and the kill path for a
        // faulting user process overran both into unmapped memory; any #PF
        // taken inside the handler would also have restarted at the same IST
        // top over the outer handler's frames. A kernel stack overflow, which
        // cannot push a #PF frame, still reaches the double-fault handler on
        // its own IST stack.
        idt.page_fault.set_handler_fn(page_fault_handler);

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
                log::error!(
                    "INVALID timer_interrupt_entry address: {:#x}",
                    timer_entry_low
                );
                // For now, use the low address directly - it should work since we preserve PML4[0]
                log::warn!("Using low-half address for timer entry (temporary workaround)");
                idt[InterruptIndex::Timer.as_u8()].set_handler_addr(VirtAddr::new(timer_entry_low));
                idt[RESCHEDULE_VECTOR].set_handler_addr(VirtAddr::new(timer_entry_low));
            } else {
                let timer_entry_high = crate::memory::layout::high_alias_from_low(timer_entry_low);
                log::info!(
                    "Timer entry: low={:#x} -> high={:#x}",
                    timer_entry_low,
                    timer_entry_high
                );
                idt[InterruptIndex::Timer.as_u8()]
                    .set_handler_addr(VirtAddr::new(timer_entry_high));
                idt[RESCHEDULE_VECTOR].set_handler_addr(VirtAddr::new(timer_entry_high));
            }
        }
        idt[InterruptIndex::Keyboard.as_u8()].set_handler_fn(keyboard_interrupt_handler);
        idt[InterruptIndex::Serial.as_u8()].set_handler_fn(serial_interrupt_handler);
        idt[InterruptIndex::Irq10.as_u8()].set_handler_fn(irq10_handler);
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
                let syscall_entry_high =
                    crate::memory::layout::high_alias_from_low(syscall_entry_low);
                log::info!(
                    "Syscall entry: low={:#x} -> high={:#x}",
                    syscall_entry_low,
                    syscall_entry_high
                );
                idt[SYSCALL_INTERRUPT_ID]
                    .set_handler_addr(x86_64::VirtAddr::new(syscall_entry_high))
                    .set_privilege_level(x86_64::PrivilegeLevel::Ring3);
            }
        }

        // Log IDT gate attributes for verification
        log::info!("IDT[0x80] gate attributes:");
        let actual_syscall_addr = syscall_entry as u64;
        if actual_syscall_addr < 0x100000 || actual_syscall_addr > 0x40000000 {
            log::info!(
                "  Handler address: {:#x} (low-half, validation failed)",
                actual_syscall_addr
            );
        } else {
            let syscall_entry_high =
                crate::memory::layout::high_alias_from_low(actual_syscall_addr);
            log::info!(
                "  Handler address: {:#x} (high-half alias)",
                syscall_entry_high
            );
        }
        log::info!("  DPL (privilege level): Ring3 (allowing userspace access)");
        log::info!("  Gate type: Interrupt gate (interrupts disabled on entry)");
        log::info!("Syscall handler configured with assembly entry point");

        // Set up a generic handler for all unhandled interrupts
        for i in 32..=255 {
            if i != InterruptIndex::Timer.as_u8()
                && i != InterruptIndex::Keyboard.as_u8()
                && i != InterruptIndex::Serial.as_u8()
                && i != InterruptIndex::Irq10.as_u8()
                && i != InterruptIndex::Irq11.as_u8()
                && i != SYSCALL_INTERRUPT_ID
                && i != RESCHEDULE_VECTOR
            {
                idt[i].set_handler_fn(generic_handler);
            }
        }

        idt[crate::arch_impl::x86_64::apic::SPURIOUS_VECTOR].set_handler_fn(apic_spurious_handler);

        // TLB shootdown requests arrive as NMIs (see memory/tlb.rs). The NMI
        // has its own IST stack: it can land between a `syscall` instruction
        // and the entry stub's switch off the user stack.
        unsafe {
            idt.non_maskable_interrupt
                .set_handler_fn(crate::memory::tlb::shootdown_nmi_handler)
                .set_stack_index(gdt::NMI_IST_INDEX);
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

/// Load the IDT `init_idt` built into the executing CPU's IDTR. Part of the
/// per-CPU init every CPU runs; all CPUs share one IDT.
pub fn load_idt() {
    IDT.get().expect("IDT not built").load();
}

/// Enable shared PCI IRQs only after their devices have initialized.
pub fn enable_irq10() {
    crate::arch_impl::x86_64::irq::set_enabled(10, true);
}

pub fn enable_irq11() {
    crate::arch_impl::x86_64::irq::set_enabled(11, true);
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
        log::info!(
            "#DB (Debug Exception) from kernel at {:#x}",
            stack_frame.instruction_pointer.as_u64()
        );
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

    // Whole lines under SERIAL1 only: with several CPUs online, bytes written
    // straight to the port land inside another CPU's line.
    crate::serial_println!("BP_HANDLER_ENTRY!");

    // Enter exception context - use preempt_disable for exceptions (not IRQs)
    crate::serial_println!("About to call preempt_disable from BP handler");
    crate::per_cpu::preempt_disable();
    crate::serial_println!("Called preempt_disable from BP handler");

    // Parse the frame structure
    // Frame layout: [r15,r14,...,rax,error_code,RIP,CS,RFLAGS,RSP,SS]
    unsafe {
        let frame = frame_ptr;
        let rip_ptr = frame.offset(16); // Skip 15 regs + error code
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
    crate::serial_println!(
        "[DIAG:DOUBLEFAULT] RIP: {:#x}",
        stack_frame.instruction_pointer.as_u64()
    );
    crate::serial_println!("[DIAG:DOUBLEFAULT] CS: {:#x}", stack_frame.code_segment.0);
    crate::serial_println!(
        "[DIAG:DOUBLEFAULT] RFLAGS: {:#x}",
        stack_frame.cpu_flags.bits()
    );
    crate::serial_println!(
        "[DIAG:DOUBLEFAULT] RSP (frame): {:#x}",
        stack_frame.stack_pointer.as_u64()
    );
    crate::serial_println!("[DIAG:DOUBLEFAULT] RSP (actual): {:#x}", actual_rsp);
    crate::serial_println!("[DIAG:DOUBLEFAULT] SS: {:#x}", stack_frame.stack_segment.0);
    crate::serial_println!("[DIAG:DOUBLEFAULT] CR2: {:#x}", cr2);
    crate::serial_println!("[DIAG:DOUBLEFAULT] CR3: {:#x}", cr3);
    crate::serial_println!("[DIAG:DOUBLEFAULT] ==============================");

    // Raw serial output FIRST to confirm we're in DF handler
    unsafe {
        core::arch::asm!(
            "mov dx, 0x3F8",
            "mov al, 0x44", // 'D' for Double Fault
            "out dx, al",
            "mov al, 0x46", // 'F'
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
    log::error!(
        "RSP (from frame): {:#x}",
        stack_frame.stack_pointer.as_u64()
    );
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

extern "x86-interrupt" fn keyboard_interrupt_handler(stack_frame: InterruptStackFrame) {
    // A trap from Ring 3 is system time until it returns there.
    let _trap_time = crate::task::thread::TrapTime::enter(stack_frame.code_segment.0 & 3 == 3);
    use x86_64::instructions::port::Port;

    // Read scancode from keyboard controller
    let scancode: u8 = unsafe {
        let mut kb_port: Port<u8> = Port::new(0x60);
        kb_port.read()
    };

    // Enter hardware IRQ context
    crate::per_cpu::irq_enter();

    // Convert F-key scancodes to VT100 escape sequences for userspace
    if let Some(seq) = scancode_to_fkey_escape(scancode) {
        for &b in seq {
            let _ = crate::tty::driver::push_char_nonblock(b);
        }
    } else if let Some(event) = crate::keyboard::process_scancode(scancode) {
        // Process scancode to get key event
        if let Some(character) = event.character {
            let c = character as u8;
            // Route through TTY
            let _ = crate::tty::driver::push_char_nonblock(c);
        }
    }

    crate::arch_impl::x86_64::irq::eoi(InterruptIndex::Keyboard.as_u8());

    crate::per_cpu::irq_exit();
}

/// Convert PS/2 F-key scancodes to VT100 escape sequences.
/// Returns None for non-F-key scancodes so regular processing continues.
fn scancode_to_fkey_escape(scancode: u8) -> Option<&'static [u8]> {
    match scancode {
        0x3B => Some(b"\x1bOP"),   // F1
        0x3C => Some(b"\x1bOQ"),   // F2
        0x3D => Some(b"\x1bOR"),   // F3
        0x3E => Some(b"\x1bOS"),   // F4
        0x3F => Some(b"\x1b[15~"), // F5
        0x40 => Some(b"\x1b[17~"), // F6
        0x41 => Some(b"\x1b[18~"), // F7
        0x42 => Some(b"\x1b[19~"), // F8
        0x43 => Some(b"\x1b[20~"), // F9
        0x44 => Some(b"\x1b[21~"), // F10
        _ => None,
    }
}

extern "x86-interrupt" fn serial_interrupt_handler(stack_frame: InterruptStackFrame) {
    // A trap from Ring 3 is system time until it returns there.
    let _trap_time = crate::task::thread::TrapTime::enter(stack_frame.code_segment.0 & 3 == 3);
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

    crate::arch_impl::x86_64::irq::eoi(InterruptIndex::Serial.as_u8());

    // Exit hardware IRQ context
    crate::per_cpu::irq_exit();
}

/// IRQ 10 shared PCI handler (network, storage and sound)
///
/// CRITICAL: This handler must be extremely fast. No logging, no allocations.
/// Target: <1000 cycles total.
extern "x86-interrupt" fn irq10_handler(stack_frame: InterruptStackFrame) {
    // A trap from Ring 3 is system time until it returns there.
    let _trap_time = crate::task::thread::TrapTime::enter(stack_frame.code_segment.0 & 3 == 3);
    // Enter hardware IRQ context
    crate::per_cpu::irq_enter();

    if crate::drivers::ahci::ahci_irq() == 10 {
        crate::drivers::ahci::handle_interrupt();
    }
    dispatch_virtio_block_interrupts(10);
    dispatch_nvme_interrupts();
    dispatch_virtio_sound_interrupts();

    // Dispatch to E1000 network if initialized
    crate::drivers::e1000::handle_interrupt();
    crate::drivers::virtio::net_legacy::handle_interrupt(10);
    crate::drivers::rtl8139::handle_interrupt(10);

    // Send EOI to both PICs (IRQ 10 is on PIC2)
    crate::arch_impl::x86_64::irq::eoi(InterruptIndex::Irq10.as_u8());

    // Exit hardware IRQ context
    crate::per_cpu::irq_exit();
}

/// IRQ 11 shared PCI handler (network, storage and sound)
///
/// CRITICAL: This handler must be extremely fast. No logging, no allocations.
/// Target: <1000 cycles total.
extern "x86-interrupt" fn irq11_handler(stack_frame: InterruptStackFrame) {
    // A trap from Ring 3 is system time until it returns there.
    let _trap_time = crate::task::thread::TrapTime::enter(stack_frame.code_segment.0 & 3 == 3);
    // Enter hardware IRQ context
    crate::per_cpu::irq_enter();

    if crate::drivers::ahci::ahci_irq() == 11 {
        crate::drivers::ahci::handle_interrupt();
    }
    dispatch_virtio_block_interrupts(11);
    dispatch_nvme_interrupts();
    dispatch_virtio_sound_interrupts();

    // Also check E1000 on IRQ 11 - some QEMU configurations route E1000 here
    crate::drivers::e1000::handle_interrupt();
    crate::drivers::virtio::net_legacy::handle_interrupt(11);
    crate::drivers::rtl8139::handle_interrupt(11);

    // Send EOI to both PICs (IRQ 11 is on PIC2)
    crate::arch_impl::x86_64::irq::eoi(InterruptIndex::Irq11.as_u8());

    // Exit hardware IRQ context
    crate::per_cpu::irq_exit();
}

#[inline]
fn dispatch_virtio_block_interrupts(irq: u8) {
    // Poll only the delivered line's devices. Each ISR read is a port access,
    // and a device on the other line is serviced by that line's own vector.
    for index in 0..4 {
        let Some(device) = crate::drivers::virtio::block::get_device_by_index(index) else {
            break;
        };
        device.handle_interrupt_on_line(irq);
    }
}

#[inline]
fn dispatch_nvme_interrupts() {
    // NVMe controllers share the same INTx lines; each drains its own
    // completion queue, which also deasserts its pin.
    crate::drivers::nvme::handle_interrupt();
}

#[inline]
fn dispatch_virtio_sound_interrupts() {
    crate::drivers::virtio::sound::handle_interrupt();
}

/// Raise fault signal `sig` with `info` for the thread this CPU was running
/// in Ring 3, with the process manager held, as Linux's force_sig_fault.
/// Returns whether a handler will run for it, or None when the thread has no
/// process.
fn raise_user_fault_signal_locked(
    manager: &mut crate::process::ProcessManager,
    sig: u32,
    info: crate::signal::types::SigInfo,
) -> Option<bool> {
    crate::per_cpu::current_thread_id_lock_free()
        .and_then(|tid| manager.find_process_by_thread_mut(tid))
        .map(|(_, process)| crate::signal::delivery::raise_fault_signal(process, sig, info))
}

/// A Ring 3 fault the kernel does not resolve, other than #PF and #GP: raise
/// `sig` with `info` for the thread. When a handler will run, this CPU is
/// sent the reschedule vector, whose return path delivers the signal before
/// the instruction runs again. Otherwise the default action ends the process
/// (`end_faulting_user_thread`) and the exception returns to the idle loop.
/// A busy process manager leaves the instruction to fault again.
///
/// Unlike an assembly interrupt entry, x86-interrupt does not switch GS: the
/// kernel's is taken here, and the user's restored for a return to Ring 3.
fn user_fault(stack_frame: &mut InterruptStackFrame, sig: u32, info: crate::signal::types::SigInfo) {
    unsafe {
        core::arch::asm!("swapgs", options(nostack, preserves_flags));
    }
    let caught = crate::process::try_manager().map(|mut guard| {
        guard
            .as_mut()
            .and_then(|manager| raise_user_fault_signal_locked(manager, sig, info))
    });
    match caught {
        Some(Some(false)) => {
            crate::per_cpu::preempt_disable();
            end_faulting_user_thread(
                stack_frame,
                crate::per_cpu::current_thread_id_lock_free(),
                crate::signal::delivery::fault_death_exit_code(sig),
                crate::tracing::providers::sched::DispatchAbandonSite::ExceptionUserFault,
            );
            // The frame returns to the idle loop, with the kernel's GS.
            return;
        }
        Some(Some(true)) => crate::task::scheduler::retry_after_interrupts_x86(),
        Some(None) | None => {}
    }
    unsafe {
        core::arch::asm!("swapgs", options(nostack, preserves_flags));
    }
}

/// End the thread this CPU was running in Ring 3 when its fault's signal
/// takes its default action. The process exit, with status `exit_code`,
/// runs in the fault-exit kernel thread, not here in exception
/// context (#511): it closes descriptors and wakes threads that may be
/// running on other CPUs, ends the rest of the thread group, and it can
/// block. The thread is made non-runnable now so nothing dispatches it
/// again, and the frame is rewritten to return to the idle loop.
///
/// Called with the kernel's GS and with the exception's preempt_disable() in
/// force, which this releases.
fn end_faulting_user_thread(
    stack_frame: &mut InterruptStackFrame,
    thread_id: Option<u64>,
    exit_code: i32,
    site: crate::tracing::providers::sched::DispatchAbandonSite,
) {
    crate::serial_println!(
        "[TC2DIAG] end_faulting_user_thread tid={:?} code={} rip={:#x} cr2={:#x}",
        thread_id,
        exit_code,
        stack_frame.instruction_pointer.as_u64(),
        x86_64::registers::control::Cr2::read_raw()
    );
    if let Some(thread_id) = thread_id {
        if !crate::task::process_task::defer_fault_exit(thread_id, exit_code) {
            panic!("No memory to queue fault exit");
        }
        crate::task::scheduler::terminate_thread(thread_id);
    }

    // Re-enable preemption before scheduling
    crate::per_cpu::preempt_enable();

    // Force a reschedule to pick up the next thread
    crate::task::scheduler::set_need_resched();

    // Switch CR3 back to kernel page table
    unsafe {
        use x86_64::registers::control::Cr3;
        use x86_64::structures::paging::PhysFrame;
        let kernel_cr3 = crate::per_cpu::get_kernel_cr3();
        if kernel_cr3 != 0 {
            crate::memory::tlb::note_root_load(kernel_cr3);
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
    // #772 diagnostics: this vector ends a dispatch without saving a
    // context and without touching the dispatch mark.
    crate::tracing::providers::sched::trace_dispatch_abandon(site);

    // CR3 is already the kernel table. Rewrite the frame last, using the
    // scheduler-owned idle thread stack rather than the dying thread's stack.
    context_switch::setup_idle_return(stack_frame);
}

extern "x86-interrupt" fn divide_by_zero_handler(mut stack_frame: InterruptStackFrame) {
    // A trap from Ring 3 is system time until it returns there.
    let _trap_time = crate::task::thread::TrapTime::enter(stack_frame.code_segment.0 & 3 == 3);
    if stack_frame.code_segment.0 & 3 == 3 {
        let rip = stack_frame.instruction_pointer.as_u64();
        user_fault(
            &mut stack_frame,
            crate::signal::constants::SIGFPE,
            crate::signal::types::SigInfo::fault(crate::signal::constants::FPE_INTDIV, rip)
                .from_trap(0, 0),
        );
        return;
    }
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

extern "x86-interrupt" fn invalid_opcode_handler(mut stack_frame: InterruptStackFrame) {
    // A trap from Ring 3 is system time until it returns there.
    let _trap_time = crate::task::thread::TrapTime::enter(stack_frame.code_segment.0 & 3 == 3);
    if stack_frame.code_segment.0 & 3 == 3 {
        let rip = stack_frame.instruction_pointer.as_u64();
        user_fault(
            &mut stack_frame,
            crate::signal::constants::SIGILL,
            crate::signal::types::SigInfo::fault(crate::signal::constants::ILL_ILLOPN, rip)
                .from_trap(6, 0),
        );
        return;
    }
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

/// #MF: an unmasked x87 exception, reported at the next x87 instruction. From
/// Ring 3, SIGFPE with the si_code of the flagged exception and si_addr at
/// the reporting instruction, as Linux reports it. The faulting thread owns
/// this CPU's x87 registers, which the kernel never uses (`fpu`).
extern "x86-interrupt" fn x87_floating_point_handler(mut stack_frame: InterruptStackFrame) {
    // A trap from Ring 3 is system time until it returns there.
    let _trap_time = crate::task::thread::TrapTime::enter(stack_frame.code_segment.0 & 3 == 3);
    if stack_frame.code_segment.0 & 3 == 3 {
        let rip = stack_frame.instruction_pointer.as_u64();
        let code = crate::arch_impl::x86_64::fpu::FpuState::capture().x87_fault_code();
        user_fault(
            &mut stack_frame,
            crate::signal::constants::SIGFPE,
            crate::signal::types::SigInfo::fault(code, rip).from_trap(16, 0),
        );
        return;
    }
    panic!("x87 floating-point exception in the kernel at {:#x}", stack_frame.instruction_pointer.as_u64());
}

/// #XM: an unmasked SSE exception. From Ring 3, SIGFPE as for #MF, from MXCSR.
extern "x86-interrupt" fn simd_floating_point_handler(mut stack_frame: InterruptStackFrame) {
    // A trap from Ring 3 is system time until it returns there.
    let _trap_time = crate::task::thread::TrapTime::enter(stack_frame.code_segment.0 & 3 == 3);
    if stack_frame.code_segment.0 & 3 == 3 {
        let rip = stack_frame.instruction_pointer.as_u64();
        let code = crate::arch_impl::x86_64::fpu::FpuState::capture().simd_fault_code();
        user_fault(
            &mut stack_frame,
            crate::signal::constants::SIGFPE,
            crate::signal::types::SigInfo::fault(code, rip).from_trap(19, 0),
        );
        return;
    }
    panic!("SIMD floating-point exception in the kernel at {:#x}", stack_frame.instruction_pointer.as_u64());
}

/// #AC: a misaligned access with alignment checking on, which only Ring 3 can
/// enable. SIGBUS, BUS_ADRALN; the CPU does not report the address.
extern "x86-interrupt" fn alignment_check_handler(mut stack_frame: InterruptStackFrame, error_code: u64) {
    // A trap from Ring 3 is system time until it returns there.
    let _trap_time = crate::task::thread::TrapTime::enter(stack_frame.code_segment.0 & 3 == 3);
    if stack_frame.code_segment.0 & 3 == 3 {
        user_fault(
            &mut stack_frame,
            crate::signal::constants::SIGBUS,
            crate::signal::types::SigInfo::fault(crate::signal::constants::BUS_ADRALN, 0)
                .from_trap(17, error_code),
        );
        return;
    }
    panic!("alignment check in the kernel at {:#x}", stack_frame.instruction_pointer.as_u64());
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
/// Every page-table edit here happens under PROCESS_MANAGER. No x86 holder can be
/// switched out (`manager()` holds a preempt brake, the other acquisitions mask
/// interrupts), so when the lock is held on this CPU the fault interrupted the
/// holding section itself (a kernel write to user memory under PM, such as
/// signal-frame setup during a dispatch), and the edit runs on that section's
/// behalf through `handle_cow_direct`. Otherwise the holder is running on
/// another CPU and the fault polls for it (`fault_manager`).
/// Copy-on-Write statistics - re-export from architecture-independent module
pub use crate::memory::cow_stats;

/// Why `fault_manager` returned without the process manager.
enum FaultManagerUnavailable {
    /// This CPU holds it: the fault interrupted the holding section, which
    /// cannot be waited for.
    HeldHere(crate::process::PmHeldOnThisCpu),
    /// Another CPU held it for the whole poll. The faulting code had
    /// interrupts enabled, so the fault returns and the instruction runs again
    /// once pending interrupts have been taken.
    Busy,
}

/// The process manager for resolving a page fault.
///
/// A holder on another CPU is polled for with `poll_manager`, the bounded
/// wait an interrupt return uses: the lock is taken at the holder's next
/// release, and the bound keeps an interrupt that holder may need from this
/// CPU from waiting longer than that. When the poll runs out, code that
/// faulted with interrupts enabled returns and faults again
/// (`FaultManagerUnavailable::Busy`), dropping the handler's preempt count, so
/// a busy lock never keeps the thread from being switched out. Code that
/// faulted with interrupts masked could not take an interrupt or be switched
/// out anyway, and keeps polling.
///
/// Trying the lock once per fault, as the handler used to, lost almost every
/// time against CPUs that take it for every switch: a process of a ring
/// passing a token with sched_yield faulted on its first copy-on-write write
/// after fork for whole quanta, keeping its CPU and the token, and the ring
/// stopped (#1265).
fn fault_manager(
    may_retry: bool,
) -> Result<crate::process::TryProcessManagerGuard, FaultManagerUnavailable> {
    loop {
        if let Some(held) = crate::process::pm_held_on_this_cpu() {
            return Err(FaultManagerUnavailable::HeldHere(held));
        }
        if let Some(guard) = crate::process::poll_manager() {
            return Ok(guard);
        }
        if may_retry {
            return Err(FaultManagerUnavailable::Busy);
        }
    }
}

fn handle_cow_fault(
    faulting_addr: VirtAddr,
    error_code: PageFaultErrorCode,
    cr3: u64,
    may_retry: bool,
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

    cow_stats::TOTAL_FAULTS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);

    match fault_manager(may_retry) {
        Ok(mut guard) => {
            cow_stats::MANAGER_PATH.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            handle_cow_with_manager(&mut guard, faulting_addr, cr3)
        }
        Err(FaultManagerUnavailable::HeldHere(held)) => {
            cow_stats::DIRECT_PATH.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            handle_cow_direct(&held, faulting_addr, cr3)
        }
        Err(FaultManagerUnavailable::Busy) => true,
    }
}

/// Resolve a fault in a private file mapping (`memory::file_map`). With
/// `user_thread`, an access that cannot complete raises its signal in that
/// thread's process. A fault taken inside a PROCESS_MANAGER section on this
/// CPU cannot wait for it and is not a file fault.
fn file_mapping_fault(
    address: u64,
    error_code: PageFaultErrorCode,
    cr3: u64,
    user_thread: Option<u64>,
    may_retry: bool,
) -> (
    crate::memory::file_map::FaultOutcome,
    Option<crate::process::TryProcessManagerGuard>,
) {
    use crate::memory::file_map::{handle_fault, Access, FaultOutcome};
    if error_code.contains(PageFaultErrorCode::MALFORMED_TABLE) {
        return (FaultOutcome::NotFile, None);
    }
    let access = if error_code.contains(PageFaultErrorCode::INSTRUCTION_FETCH) {
        Access::Execute
    } else if error_code.contains(PageFaultErrorCode::CAUSED_BY_WRITE) {
        Access::Write
    } else {
        Access::Read
    };
    match crate::memory::anon_map::handle_fault(cr3, address, access, user_thread, || {
        fault_manager(may_retry).ok()
    }) {
        FaultOutcome::NotFile => {}
        outcome => return (outcome, None),
    }
    match fault_manager(may_retry) {
        Ok(mut guard) => {
            let outcome = match guard.as_mut() {
                Some(manager) => handle_fault(manager, cr3, address, access, user_thread),
                None => FaultOutcome::NotFile,
            };
            // Keep the guard for signal disposition or the generic user fault;
            // no second poll is needed after the mapping check.
            (outcome, Some(guard))
        }
        Err(FaultManagerUnavailable::HeldHere(_)) => (FaultOutcome::NotFile, None),
        // Retried as for a copy-on-write fault.
        Err(FaultManagerUnavailable::Busy) => (FaultOutcome::Resolved, None),
    }
}

/// A write protection fault on a user page that every level of `cr3`'s
/// tables already makes present, writable and user-accessible came through a
/// stale read-only translation on this CPU: a copy-on-write sole-owner upgrade
/// flushes only the CPU that made it. Drop the stale entry and report the
/// fault resolved, as x86 Linux treats a spurious write fault.
fn resolve_stale_write_translation(cr3: u64, addr: VirtAddr) -> bool {
    use x86_64::structures::paging::PageTableFlags;

    resolve_stale_translation(
        cr3,
        addr,
        PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE,
    )
}

/// Whether every level of `cr3`'s tables maps the user page at `addr` with
/// `needed`; if so, drop this CPU's entry for it and report the fault resolved.
fn resolve_stale_translation(
    cr3: u64,
    addr: VirtAddr,
    needed: x86_64::structures::paging::PageTableFlags,
) -> bool {
    use x86_64::structures::paging::{PageTable, PageTableFlags};

    let phys_offset = crate::memory::physical_memory_offset();
    let mut table_phys = cr3 & !0xfff;
    for level in (0..4).rev() {
        let index = ((addr.as_u64() >> (12 + 9 * level)) & 0x1FF) as usize;
        // SAFETY: `table_phys` is a page-table frame of the faulting address
        // space, reached from its root through present entries, and the
        // physical-memory window maps it.
        let table: &PageTable = unsafe { &*(phys_offset + table_phys).as_ptr::<PageTable>() };
        let entry = &table[index];
        let flags = entry.flags();
        if !flags.contains(needed) || (level > 0 && flags.contains(PageTableFlags::HUGE_PAGE)) {
            return false;
        }
        table_phys = entry.addr().as_u64();
    }
    x86_64::instructions::tlb::flush(addr);
    true
}

/// Handle CoW fault through the process manager (normal path)
fn handle_cow_with_manager(
    guard: &mut crate::process::TryProcessManagerGuard,
    faulting_addr: VirtAddr,
    cr3: u64,
) -> bool {
    use crate::memory::frame_allocator::{allocate_frame, deallocate_leaf_frame};
    use crate::memory::frame_metadata::frame_is_shared;
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

    if resolve_stale_write_translation(cr3, faulting_addr) {
        return true;
    }

    // Check if this is actually a CoW page
    if !is_cow_page(old_flags) {
        return false;
    }

    crate::tracing::providers::counters::count_cow_fault();

    // Check if we're the only reference - can just make it writable
    if !frame_is_shared(old_frame) {
        let new_flags = make_private_flags(old_flags);
        if page_table.update_page_flags(page, new_flags).is_err() {
            return false;
        }
        // A permission upgrade: another CPU still holding the read-only
        // translation takes a spurious write fault, which
        // `resolve_stale_write_translation` resolves, so only this CPU's
        // entry is dropped. A flush of every CPU here cost an NMI
        // round per page a parent wrote after its forked child exited.
        x86_64::instructions::tlb::flush(faulting_addr);
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

    // Update page table: unmap old, map new with writable flags. The old
    // frame's reference is released only after every CPU has dropped its
    // translation (`ReleasedLeaf::flush`), so no CPU can read it through a
    // stale entry once its other owner is free to write it.
    let new_flags = make_private_flags(old_flags);
    let old_leaf = match page_table.unmap_page_deferred(page) {
        Ok(leaf) => leaf,
        Err(_) => {
            let _ = deallocate_leaf_frame(new_frame);
            return false;
        }
    };
    if page_table.map_page(page, new_frame, new_flags).is_err() {
        let _ = page_table.map_page(page, old_frame, old_flags);
        old_leaf.flush().release();
        let _ = deallocate_leaf_frame(new_frame);
        return false;
    }
    old_leaf.flush().release();

    cow_stats::PAGES_COPIED.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    true
}

/// Handle CoW fault directly via CR3, on behalf of the PM section the fault
/// interrupted.
///
/// This function walks the page table manually and modifies entries directly.
/// It runs under PROCESS_MANAGER, held by the section this fault interrupted;
/// the `_held` proof is what makes that true, so it cannot be called otherwise.
fn handle_cow_direct(
    _held: &crate::process::PmHeldOnThisCpu,
    faulting_addr: VirtAddr,
    cr3: u64,
) -> bool {
    use crate::memory::frame_allocator::{
        acquire_leaf_mapping, allocate_frame, deallocate_leaf_frame, LeafMappingClass,
        ReturnOutcome,
    };
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

        if resolve_stale_write_translation(cr3, faulting_addr) {
            return true;
        }

        // Check if this is a CoW page
        if !is_cow_page(old_flags) {
            return false;
        }

        crate::tracing::providers::counters::count_cow_fault();

        // Check if we're the only reference
        if !frame_is_shared(old_frame) {
            // Sole owner - just update flags to make writable. A permission
            // upgrade, flushed on this CPU only, as in `handle_cow_with_manager`.
            let new_flags = make_private_flags(old_flags);
            l1_entry.set_addr(l1_entry.addr(), new_flags);
            x86_64::instructions::tlb::flush(faulting_addr);
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

        // The virtual-page custody record remains Owned across a CoW
        // replacement. Acquire the new allocator-derived reference before the
        // descriptor becomes visible, then release the old reference after.
        if acquire_leaf_mapping(new_frame) != Ok(LeafMappingClass::Owned) {
            let _ = deallocate_leaf_frame(new_frame);
            return false;
        }
        let new_flags = make_private_flags(old_flags);
        l1_entry.set_addr(new_frame.start_address(), new_flags);

        // Every CPU drops the old translation before the old frame can be
        // returned: a stale entry would keep writes landing in a freed frame.
        X86PageTableOps::flush_tlb_page(faulting_addr.as_u64());

        if frame_decref(old_frame) && deallocate_leaf_frame(old_frame) == ReturnOutcome::Returned {
            crate::trace_count!(crate::tracing::providers::teardown::LEAF_FRAMES_RETURNED);
        }

        cow_stats::PAGES_COPIED.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        true
    }
}

/// Whether `fault_addr` is in or near the user stack region, the only place
/// `handle_stack_growth` grows a stack. A stack grows downward, so its faults
/// land BELOW the current stack bottom but still within the overall region.
fn in_user_stack_growth_range(fault_addr: u64) -> bool {
    use crate::memory::layout::{
        MAX_USER_STACK_SIZE, USER_STACK_REGION_END, USER_STACK_REGION_START,
    };
    fault_addr < USER_STACK_REGION_END
        && fault_addr >= USER_STACK_REGION_START.saturating_sub(MAX_USER_STACK_SIZE)
}

/// Handle demand-paged stack growth
///
/// When a userspace process accesses memory just below its current stack bottom,
/// this function extends the stack by allocating and mapping new pages. This allows
/// stacks to grow on demand up to MAX_USER_STACK_SIZE without pre-allocating memory.
///
/// Returns true if the fault was handled (stack was grown), false otherwise.
///
/// A process manager busy on another CPU is polled for (`fault_manager`);
/// past the poll, with `may_retry`, this returns true without growing
/// anything, and the faulting instruction runs again and faults again.
fn handle_stack_growth(faulting_addr: VirtAddr, cr3: u64, may_retry: bool) -> bool {
    let fault_addr = faulting_addr.as_u64();

    if !in_user_stack_growth_range(fault_addr) {
        return false;
    }

    // A holder on this CPU is the section this fault interrupted, which
    // cannot be waited for; no stack is grown on its behalf.
    let mut guard = match fault_manager(may_retry) {
        Ok(guard) => guard,
        Err(FaultManagerUnavailable::HeldHere(_)) => return false,
        Err(FaultManagerUnavailable::Busy) => return true,
    };

    let pm = match guard.as_mut() {
        Some(pm) => pm,
        None => return false,
    };

    let (_pid, process) = match pm.find_process_by_cr3_mut(cr3) {
        Some(p) => p,
        None => return false,
    };

    // The fault must be below the current stack bottom (stack grows down).
    // Any access between the current bottom and the stack's maximum extent
    // grows it, as on Linux: a function whose frame is larger than the
    // distance to the current bottom touches its first page far below it
    // (a 128 KiB local array lands 64 KiB past a fresh 64 KiB stack).
    fault_addr < process.user_stack_bottom && process.grow_user_stack(fault_addr)
}

extern "x86-interrupt" fn page_fault_handler(
    mut stack_frame: InterruptStackFrame,
    error_code: PageFaultErrorCode,
) {
    // A trap from Ring 3 is system time until it returns there.
    let _trap_time = crate::task::thread::TrapTime::enter(stack_frame.code_segment.0 & 3 == 3);
    use x86_64::registers::control::Cr2;

    // Read CR2 and CR3 first
    let cr2 = Cr2::read().unwrap_or(x86_64::VirtAddr::zero()).as_u64();
    // Whether the faulting code had interrupts enabled, so that a fault that
    // finds the process manager busy on another CPU past `fault_manager`'s
    // poll can return and run again.
    let may_retry = stack_frame
        .cpu_flags
        .contains(x86_64::registers::rflags::RFlags::INTERRUPT_FLAG);
    let cr3 = {
        use x86_64::registers::control::Cr3;
        let (frame, _) = Cr3::read();
        frame.start_address().as_u64()
    };

    let is_potential_cow = error_code.contains(PageFaultErrorCode::PROTECTION_VIOLATION)
        && error_code.contains(PageFaultErrorCode::CAUSED_BY_WRITE);

    // A kernel fault inside the user-copy routine on a user address is the
    // syscall's bad pointer, not a kernel bug: a copy-on-write write or a
    // user-stack growth is resolved as it would be for a user-mode access and
    // the copy retried; anything else resumes at the routine's fault exit,
    // which returns EFAULT to the syscall.
    if (stack_frame.code_segment.0 & 3) == 0 {
        let rip = stack_frame.instruction_pointer.as_u64();
        if let Some(fixup) = crate::syscall::userptr::uaccess_fixup(rip, cr2) {
            crate::per_cpu::preempt_disable();
            let addr = x86_64::VirtAddr::new(cr2);
            let resolved = if is_potential_cow {
                handle_cow_fault(addr, error_code, cr3, may_retry)
            } else {
                (!error_code.contains(PageFaultErrorCode::PROTECTION_VIOLATION)
                    && handle_stack_growth(addr, cr3, may_retry))
                    || file_mapping_fault(cr2, error_code, cr3, None, may_retry).0
                        == crate::memory::file_map::FaultOutcome::Resolved
            };
            crate::per_cpu::preempt_enable();
            if !resolved {
                // SAFETY: redirects this kernel-mode frame to a label in the
                // same routine, which returns to its caller with RAX = 1.
                unsafe {
                    stack_frame.as_mut().update(|frame| {
                        frame.instruction_pointer = x86_64::VirtAddr::new(fixup);
                    });
                }
            }
            return;
        }
    }

    // An access to a private file mapping is resolved from the file's page
    // cache and retried. That includes kernel stores and loads through raw
    // user pointers outside the user-copy routine. A user access that cannot
    // complete raises SIGSEGV/SIGBUS through the process's signal
    // dispositions and is retried until the signal is taken; a kernel one
    // continues below as before.
    let mut mapping_guard = None;
    if cr2 < crate::memory::layout::USER_STACK_REGION_END {
        let from_user = (stack_frame.code_segment.0 & 3) == 3;
        crate::per_cpu::preempt_disable();
        let (outcome, guard) = file_mapping_fault(
            cr2,
            error_code,
            cr3,
            if from_user {
                crate::per_cpu::current_thread_id_lock_free()
            } else {
                None
            },
            may_retry,
        );
        crate::per_cpu::preempt_enable();
        match outcome {
            crate::memory::file_map::FaultOutcome::NotFile => {
                if from_user { mapping_guard = guard; }
            }
            crate::memory::file_map::FaultOutcome::Signal(_) if !from_user => {}
            crate::memory::file_map::FaultOutcome::Signal(sig) => {
                // Raised for the thread. A handler runs on the reschedule
                // vector's return; the default action ends the process here.
                // A process manager busy on another CPU past `fault_manager`'s
                // poll leaves the access to fault again.
                let caught = guard.or_else(|| fault_manager(may_retry).ok()).map(|guard| {
                    let thread = crate::per_cpu::current_thread_id_lock_free();
                    guard.as_ref().is_some_and(|manager| {
                        thread
                            .and_then(|tid| manager.find_process_by_thread(tid))
                            .is_none_or(|(_, process)| process.signals.get_handler(sig).is_handler())
                    })
                });
                match caught {
                    Some(true) => crate::task::scheduler::retry_after_interrupts_x86(),
                    Some(false) => {
                        crate::per_cpu::preempt_disable();
                        end_faulting_user_thread(
                            &mut stack_frame,
                            crate::per_cpu::current_thread_id_lock_free(),
                            crate::signal::delivery::fault_death_exit_code(sig),
                            crate::tracing::providers::sched::DispatchAbandonSite::ExceptionPageFault,
                        );
                    }
                    None => {}
                }
                return;
            }
            _ => return,
        }
    }

    // Resolve recoverable faults before reporting a fatal exception. Serial
    // output here serializes faulting CPUs with interrupts masked, delaying
    // concurrent handled faults before their signal handlers can run.
    crate::per_cpu::preempt_disable();
    let accessed_addr = x86_64::VirtAddr::new(cr2);

    // Check if this came from userspace
    let from_userspace = (stack_frame.code_segment.0 & 3) == 3;

    // Check if this is a guard page access
    if let Some(stack) = (!from_userspace)
        .then(|| crate::memory::stack::is_guard_page_fault(accessed_addr))
        .flatten()
    {
        log::error!("STACK OVERFLOW DETECTED!");
        log::error!("Attempted to access guard page at: {:?}", accessed_addr);
        log::error!("Stack bottom (guard page): {:?}", stack.guard_page());
        log::error!("Stack range: {:?} - {:?}", stack.bottom(), stack.top());
        log::error!("This indicates the stack has overflowed!");
        log::error!("Stack frame: {:#?}", stack_frame);

        panic!("Stack overflow - guard page accessed");
    }

    if !from_userspace || is_potential_cow || in_user_stack_growth_range(cr2) {
        drop(mapping_guard.take());
    }

    // Try to handle as Copy-on-Write fault
    // This handles writes to pages that were marked read-only during fork()
    // We check if the address is in userspace (< 0x8000_0000_0000) rather than
    // just checking if the fault came from userspace. This allows the kernel
    // to trigger CoW when writing to user memory (e.g., signal frame setup).
    let is_user_address = accessed_addr.as_u64() < crate::memory::layout::USER_STACK_REGION_END;
    if is_user_address && handle_cow_fault(accessed_addr, error_code, cr3, may_retry) {
        // CoW fault handled successfully - resume execution
        crate::per_cpu::preempt_enable();
        return;
    }

    // Try to handle as demand-paged stack growth
    // Stack growth faults are: not-present page (no PROTECTION_VIOLATION) + not instruction fetch
    // This is mutually exclusive with CoW faults (which require PROTECTION_VIOLATION)
    if from_userspace
        && !error_code.contains(PageFaultErrorCode::PROTECTION_VIOLATION)
        && !error_code.contains(PageFaultErrorCode::INSTRUCTION_FETCH)
        && handle_stack_growth(accessed_addr, cr3, may_retry)
    {
        crate::per_cpu::preempt_enable();
        return;
    }

    // A fatal user fault needs the process manager to name its process. One
    // held on another CPU past `fault_manager`'s poll leaves the fault to be
    // taken again once pending interrupts have run; probing here keeps a retry
    // from printing the diagnostics below each time.
    //
    // Holding it, the page may turn out to be mapped after all: a
    // copy-on-write break on another CPU clears the entry, flushes every CPU
    // and maps the new frame under the process manager, so another thread of
    // the process touching the page in between faults on a not-present entry.
    // When the tables now allow the access the fault was spurious, and the
    // access is retried, as Linux does when the entry is valid by the time it
    // looks.
    //
    // Otherwise the fault raises SIGSEGV for the thread. When a handler will
    // run, the reschedule vector's return path delivers it before the
    // instruction runs again; the default action ends the process below.
    if from_userspace {
        let Some(mut guard) = mapping_guard.take().or_else(|| fault_manager(may_retry).ok()) else {
            crate::per_cpu::preempt_enable();
            return;
        };
        let mut needed = x86_64::structures::paging::PageTableFlags::PRESENT
            | x86_64::structures::paging::PageTableFlags::USER_ACCESSIBLE;
        if error_code.contains(PageFaultErrorCode::CAUSED_BY_WRITE) {
            needed |= x86_64::structures::paging::PageTableFlags::WRITABLE;
        }
        let raced = !error_code.contains(PageFaultErrorCode::INSTRUCTION_FETCH)
            && accessed_addr.as_u64() < crate::memory::layout::USER_STACK_REGION_END
            && resolve_stale_translation(cr3, accessed_addr, needed);
        let caught = !raced
            && guard.as_mut().is_some_and(|manager| {
                let code = if error_code.contains(PageFaultErrorCode::PROTECTION_VIOLATION) {
                    crate::signal::constants::SEGV_ACCERR
                } else {
                    crate::signal::constants::SEGV_MAPERR
                };
                raise_user_fault_signal_locked(
                    manager,
                    crate::signal::constants::SIGSEGV,
                    crate::signal::types::SigInfo::fault(code, accessed_addr.as_u64())
                        .from_trap(14, error_code.bits()),
                ) == Some(true)
            });
        drop(guard);
        if caught {
            crate::task::scheduler::retry_after_interrupts_x86();
        }
        if raced || caught {
            crate::per_cpu::preempt_enable();
            return;
        }
        crate::serial_println!(
            "[TC2DIAG] user page fault kills tid={:?} cr2={:#x} err={:#x} rip={:#x} cr3={:#x} cow={}",
            crate::per_cpu::current_thread_id_lock_free(),
            cr2,
            error_code.bits(),
            stack_frame.instruction_pointer.as_u64(),
            cr3,
            is_potential_cow
        );
        end_faulting_user_thread(
            &mut stack_frame,
            crate::per_cpu::current_thread_id_lock_free(),
            crate::signal::delivery::fault_death_exit_code(crate::signal::constants::SIGSEGV),
            crate::tracing::providers::sched::DispatchAbandonSite::ExceptionPageFault,
        );
        return;
    }

    crate::serial_println!("EXCEPTION: PAGE FAULT");

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
        crate::serial_println!(
            "  Error code: {:#x} (P={} W={} U={} I={} PK={})",
            error_code.bits(),
            if error_code.contains(PageFaultErrorCode::PROTECTION_VIOLATION) {
                1
            } else {
                0
            },
            if error_code.contains(PageFaultErrorCode::CAUSED_BY_WRITE) {
                1
            } else {
                0
            },
            if error_code.contains(PageFaultErrorCode::USER_MODE) {
                1
            } else {
                0
            },
            if error_code.contains(PageFaultErrorCode::INSTRUCTION_FETCH) {
                1
            } else {
                0
            },
            if error_code.contains(PageFaultErrorCode::PROTECTION_KEY) {
                1
            } else {
                0
            }
        );
        crate::serial_println!(
            "  CS:RIP: {:#x}:{:#x}",
            stack_frame.code_segment.0,
            stack_frame.instruction_pointer.as_u64()
        );
        crate::serial_println!(
            "  SS:RSP: {:#x}:{:#x}",
            stack_frame.stack_segment.0,
            stack_frame.stack_pointer.as_u64()
        );
        crate::serial_println!("  RFLAGS: {:#x}", stack_frame.cpu_flags.bits());
        crate::serial_println!("  Current RSP: {:#x}, RBP: {:#x}", rsp, rbp);

        // Determine what PML4 entry the fault address belongs to
        let pml4_index = (accessed_addr.as_u64() >> 39) & 0x1FF;
        crate::serial_println!(
            "  Fault address PML4 index: {} (PML4[{}])",
            pml4_index,
            pml4_index
        );

        // Also log which PML4 entry the faulting instruction belongs to
        let rip_pml4_index = (stack_frame.instruction_pointer.as_u64() >> 39) & 0x1FF;
        crate::serial_println!(
            "  RIP address PML4 index: {} (PML4[{}])",
            rip_pml4_index,
            rip_pml4_index
        );

        // Check if this is instruction fetch vs data access
        if error_code.contains(PageFaultErrorCode::INSTRUCTION_FETCH) {
            crate::serial_println!(
                "  INSTRUCTION FETCH fault - code page not executable or not present!"
            );
        } else if error_code.contains(PageFaultErrorCode::CAUSED_BY_WRITE) {
            crate::serial_println!("  WRITE fault - page not writable or not present!");
        } else {
            crate::serial_println!("  READ fault - page not readable or not present!");
        }
    }

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
    {
        // Kernel page fault - this is a bug, panic
        panic!(
            "Kernel page fault at {:#x} (error: {:?})",
            accessed_addr.as_u64(),
            error_code
        );
    }
}

extern "x86-interrupt" fn generic_handler(stack_frame: InterruptStackFrame) {
    // A trap from Ring 3 is system time until it returns there.
    let _trap_time = crate::task::thread::TrapTime::enter(stack_frame.code_segment.0 & 3 == 3);
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
    crate::arch_impl::x86_64::irq::eoi(PIC_2_OFFSET + 7);

    // Exit hardware IRQ context
    crate::per_cpu::irq_exit();
}

extern "x86-interrupt" fn stack_segment_fault_handler(
    mut stack_frame: InterruptStackFrame,
    error_code: u64,
) {
    // A trap from Ring 3 is system time until it returns there.
    let _trap_time = crate::task::thread::TrapTime::enter(stack_frame.code_segment.0 & 3 == 3);
    // From Ring 3 (a non-canonical stack address, say): SIGBUS, SI_KERNEL,
    // as Linux reports it.
    if stack_frame.code_segment.0 & 3 == 3 {
        user_fault(
            &mut stack_frame,
            crate::signal::constants::SIGBUS,
            crate::signal::types::SigInfo::kernel().from_trap(12, error_code),
        );
        return;
    }

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

    log::error!(
        "  CS: {:#x} (RPL={})",
        stack_frame.code_segment.0,
        stack_frame.code_segment.0 & 3
    );
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
    // A trap from Ring 3 is system time until it returns there.
    let _trap_time = crate::task::thread::TrapTime::enter(stack_frame.code_segment.0 & 3 == 3);
    // IRETQ can reject a user return frame while CS still names Ring 0.
    // Treat that as a fault of the returning user, not a kernel exception.
    // Both syscall entries share this IRETQ fallback after restoring user GS.
    extern "C" {
        fn syscall_return_to_userspace(rip: u64, rsp: u64, rflags: u64) -> !;
    }
    let rip = stack_frame.instruction_pointer.as_u64();
    let entry = super::syscall::entry_address(syscall_entry as u64);
    let end = super::syscall::entry_address(syscall_return_to_userspace as u64);
    let fault_on_user_return = stack_frame.code_segment.0 & 3 == 0
        && rip >= entry
        && rip < end
        && unsafe { core::ptr::read_unaligned(rip as *const u16) == 0xcf48 }
        && unsafe { *stack_frame.stack_pointer.as_ptr::<u64>().add(1) & 3 == 3 };
    if fault_on_user_return {
        unsafe {
            core::arch::asm!("swapgs", options(nostack, preserves_flags));
        }
    }

    // DIAGNOSTIC OUTPUT AT THE VERY START
    let cr3 = {
        use x86_64::registers::control::Cr3;
        let (frame, _) = Cr3::read();
        frame.start_address().as_u64()
    };

    crate::serial_println!("[DIAG:GPF] ==============================");
    crate::serial_println!("[DIAG:GPF] Error code: {:#x}", error_code);
    crate::serial_println!(
        "[DIAG:GPF] RIP: {:#x}",
        stack_frame.instruction_pointer.as_u64()
    );
    crate::serial_println!("[DIAG:GPF] CS: {:#x}", stack_frame.code_segment.0);
    crate::serial_println!("[DIAG:GPF] RFLAGS: {:#x}", stack_frame.cpu_flags.bits());
    crate::serial_println!("[DIAG:GPF] RSP: {:#x}", stack_frame.stack_pointer.as_u64());
    crate::serial_println!("[DIAG:GPF] SS: {:#x}", stack_frame.stack_segment.0);
    crate::serial_println!("[DIAG:GPF] CR3: {:#x}", cr3);
    crate::serial_println!("[DIAG:GPF] ==============================");

    // Increment preempt count on exception entry
    crate::per_cpu::preempt_disable();

    // Check if this came from userspace
    let from_userspace = (stack_frame.code_segment.0 & 3) == 3 || fault_on_user_return;

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
    log::error!(
        "  Decoded: external={}, table={} ({}), index={}, selector={:#x}",
        external,
        table,
        table_name,
        index,
        selector
    );

    // Check if this might be an IRETQ failure
    if !from_userspace && stack_frame.instruction_pointer.as_u64() < 0x1000_0000 {
        log::error!("  💥 LIKELY IRETQ FAILURE - fault during return to userspace!");
        log::error!(
            "  Problematic selector: {:#x} from {}",
            selector,
            table_name
        );
        if selector == 0x33 {
            log::error!("  Issue with user CS (0x33) - check GDT entry, L bit, DPL");
        } else if selector == 0x2b {
            log::error!("  Issue with user SS (0x2b) - check GDT entry, DPL");
        }
    }

    log::error!(
        "  CS: {:#x} (RPL={})",
        stack_frame.code_segment.0,
        stack_frame.code_segment.0 & 3
    );
    log::error!("  RIP: {:#x}", stack_frame.instruction_pointer.as_u64());

    // Enhanced logging for userspace GPFs (Ring 3 privilege violation tests)
    if from_userspace {
        log::error!("  GPF from USERSPACE (Ring 3)");

        // Try to identify which instruction caused the fault
        {
            let rip = stack_frame.instruction_pointer.as_u64() as *const u8;
            let byte = unsafe { core::ptr::read_volatile(rip) };
            match byte {
                0xfa => {
                    log::info!("  ✓ CLI instruction detected (0xfa) - expected privilege violation")
                }
                0xf4 => {
                    log::info!("  ✓ HLT instruction detected (0xf4) - expected privilege violation")
                }
                0x0f => {
                    // Check for MOV CR3 (0x0f 0x22 0xd8)
                    let byte2 = unsafe { core::ptr::read_volatile(rip.offset(1)) };
                    if byte2 == 0x22 {
                        log::info!("  ✓ MOV CR3 instruction detected (0x0f 0x22) - expected privilege violation");
                    }
                }
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

        // The thread this CPU was running, or returning to, took the fault;
        // CR3 names only the address space, as in the page-fault vector.
        let interrupted = crate::per_cpu::current_thread_id_lock_free();
        let mut faulting_thread_id: Option<u64> = None;
        let mut find_faulting_thread = |pm: &mut crate::process::ProcessManager| {
            if let Some((pid, process)) =
                interrupted.and_then(|tid| pm.find_process_by_thread_mut(tid))
            {
                faulting_thread_id = interrupted;
                log::error!(
                    "Killing process {} (PID {}) due to GPF (thread {:?}, CR3={:#x})",
                    process.name,
                    pid.as_u64(),
                    interrupted,
                    cr3
                );
            } else {
                log::error!(
                    "Could not find the process of thread {:?} (CR3={:#x}) - cannot terminate",
                    interrupted,
                    cr3
                );
            }
        };

        // As in the page-fault vector: a Ring 3 fault that finds the process
        // manager busy on another CPU is taken again once pending interrupts
        // have run. The IRETQ fault on a return to Ring 3 runs with them
        // masked and has no such window, so it waits for the lock.
        //
        // A #GP in Ring 3 raises SIGSEGV for the thread (si_code SI_KERNEL, as
        // Linux). When a handler will run, the reschedule vector's return
        // path delivers it before the instruction runs again; the default
        // action ends the process below. A faulting IRETQ has no user
        // instruction to retry, and ends the process.
        if fault_on_user_return {
            crate::process::with_process_manager(find_faulting_thread);
        } else {
            let Some(mut guard) = crate::process::try_manager() else {
                crate::per_cpu::preempt_enable();
                return;
            };
            let caught = guard.as_mut().is_some_and(|pm| {
                raise_user_fault_signal_locked(
                    pm,
                    crate::signal::constants::SIGSEGV,
                    crate::signal::types::SigInfo::kernel().from_trap(13, error_code),
                ) == Some(true)
            });
            if caught {
                drop(guard);
                crate::task::scheduler::retry_after_interrupts_x86();
                crate::per_cpu::preempt_enable();
                return;
            }
            if let Some(pm) = guard.as_mut() {
                find_faulting_thread(pm);
            }
        }

        // Deferred to the fault-exit kernel thread, as in the page-fault
        // vector (#511).
        if let Some(thread_id) = faulting_thread_id {
            if !crate::task::process_task::defer_fault_sigsegv_exit(thread_id) {
                log::error!("Fault exit of thread {} lost: no memory to queue it", thread_id);
            }
            crate::task::scheduler::terminate_thread(thread_id);
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
                crate::memory::tlb::note_root_load(kernel_cr3);
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
        // #772 diagnostics: same as the page-fault vector above.
        crate::tracing::providers::sched::trace_dispatch_abandon(
            crate::tracing::providers::sched::DispatchAbandonSite::ExceptionGeneralProtection,
        );

        // CR3 is already the kernel table. Rewrite the frame last, using the
        // scheduler-owned idle thread stack rather than the exception IST stack.
        context_switch::setup_idle_return(&mut stack_frame);

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
#[allow(dead_code)] // Used in kernel_main_continue (conditionally compiled)
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
                return (
                    false,
                    handler_addr,
                    "Handler address looks invalid (in low memory)",
                );
            }

            (true, handler_addr, "Handler address valid")
        }
    } else {
        (false, 0, "IDT not initialized")
    }
}

/// Check if interrupts are currently enabled
#[allow(dead_code)] // Used in kernel_main_continue (conditionally compiled)
pub fn are_interrupts_enabled() -> bool {
    x86_64::instructions::interrupts::are_enabled()
}

/// Validate that the PIC has IRQ0 (timer) unmasked
/// Returns (is_unmasked, mask_value, description)
#[allow(dead_code)] // Used in kernel_main_continue (conditionally compiled)
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

// LAPIC spurious interrupts have no in-service bit and must not receive EOI.
extern "x86-interrupt" fn apic_spurious_handler(_frame: InterruptStackFrame) {}
