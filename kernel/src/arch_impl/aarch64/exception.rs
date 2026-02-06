//! ARM64 exception handlers.
//!
//! These handlers are called from the assembly exception vector table.
//! They process synchronous exceptions (syscalls, page faults, etc.) and IRQs.
//!
//! For syscalls (SVC from EL0), the handler delegates to the dedicated
//! syscall entry module (`syscall_entry.rs`) which provides preemption
//! handling, signal delivery, and context switch support.

#![allow(dead_code)]

use crate::arch_impl::aarch64::gic;
use crate::arch_impl::aarch64::exception_frame::Aarch64ExceptionFrame;
use crate::arch_impl::aarch64::syscall_entry::rust_syscall_handler_aarch64;
use crate::arch_impl::traits::SyscallFrame;

/// ARM64 syscall result type (mirrors x86_64 version)
#[derive(Debug)]
pub enum SyscallResult {
    Ok(u64),
    Err(u64),
}

/// Exception Syndrome Register (ESR_EL1) exception class values
mod exception_class {
    pub const UNKNOWN: u32 = 0b000000;
    pub const SVC_AARCH64: u32 = 0b010101;  // SVC instruction (syscall)
    pub const INSTRUCTION_ABORT_LOWER: u32 = 0b100000;
    pub const INSTRUCTION_ABORT_SAME: u32 = 0b100001;
    pub const DATA_ABORT_LOWER: u32 = 0b100100;
    pub const DATA_ABORT_SAME: u32 = 0b100101;
    pub const SP_ALIGNMENT: u32 = 0b100110;
    pub const FP_EXCEPTION: u32 = 0b101100;
    pub const SERROR: u32 = 0b101111;
    pub const BREAKPOINT_LOWER: u32 = 0b110000;
    pub const BREAKPOINT_SAME: u32 = 0b110001;
    pub const SOFTWARE_STEP_LOWER: u32 = 0b110010;
    pub const SOFTWARE_STEP_SAME: u32 = 0b110011;
    pub const WATCHPOINT_LOWER: u32 = 0b110100;
    pub const WATCHPOINT_SAME: u32 = 0b110101;
    pub const BRK_AARCH64: u32 = 0b111100;  // BRK instruction
}

/// Handle synchronous exceptions (syscalls, page faults, etc.)
///
/// Called from assembly with:
/// - x0 = pointer to Aarch64ExceptionFrame
/// - x1 = ESR_EL1 (Exception Syndrome Register)
/// - x2 = FAR_EL1 (Fault Address Register)
#[no_mangle]
pub extern "C" fn handle_sync_exception(frame: *mut Aarch64ExceptionFrame, esr: u64, far: u64) {
    let ec = ((esr >> 26) & 0x3F) as u32;  // Exception Class
    let iss = (esr & 0x1FFFFFF) as u32;    // Instruction Specific Syndrome

    match ec {
        exception_class::SVC_AARCH64 => {
            // Syscall - ARM64 ABI: X8=syscall number, X0-X5=args, X0=return
            // Delegate to the dedicated syscall entry module which handles:
            // - Preemption counting
            // - EL0_CONFIRMED marker
            // - Signal delivery on return
            // - Context switch checking
            let frame = unsafe { &mut *frame };

            // Check if from EL0 (userspace) - use full handler with preemption/signals
            let from_el0 = (frame.spsr & 0xF) == 0;
            if from_el0 {
                rust_syscall_handler_aarch64(frame);
            } else {
                // From EL1 (kernel) - use simple handler (shouldn't happen normally)
                handle_syscall(frame);
            }
        }

        exception_class::DATA_ABORT_LOWER | exception_class::DATA_ABORT_SAME => {
            // Try to handle as CoW fault first
            if handle_cow_fault_arm64(far, iss) {
                // CoW fault handled successfully, return to userspace
                return;
            }

            // Not a CoW fault or couldn't be handled
            let frame_ref = unsafe { &mut *frame };
            crate::serial_println!("[exception] Data abort at address {:#x}", far);
            crate::serial_println!("  ELR: {:#x}, ESR: {:#x}", frame_ref.elr, esr);
            crate::serial_println!("  ISS: {:#x} (WnR={}, DFSC={:#x})",
                iss, (iss >> 6) & 1, iss & 0x3F);

            // Check if from userspace (EL0) - SPSR[3:0] indicates source EL
            let from_el0 = (frame_ref.spsr & 0xF) == 0;
            let ttbr0: u64;
            unsafe {
                core::arch::asm!("mrs {}, ttbr0_el1", out(reg) ttbr0, options(nomem, nostack));
            }
            crate::serial_println!(
                "[DATA_ABORT] TTBR0_EL1={:#x}, FAR={:#x}, ESR={:#x}",
                ttbr0,
                far,
                esr
            );

            if from_el0 {
                // From userspace - terminate the process with SIGSEGV
                crate::serial_println!("[exception] Terminating userspace process with SIGSEGV");

                // Get current TTBR0 to find the process
                let page_table_phys = ttbr0 & !0xFFFF_0000_0000_0FFF;

                // Find and terminate the process
                let mut terminated = false;
                crate::process::with_process_manager(|pm| {
                    if let Some((pid, process)) = pm.find_process_by_cr3_mut(page_table_phys) {
                        let name = process.name.clone();
                        crate::serial_println!("[exception] Killing process {} (PID {}) due to data abort",
                            name, pid.as_u64());
                        pm.exit_process(pid, -11); // SIGSEGV exit code
                        terminated = true;
                    } else {
                        crate::serial_println!("[exception] Could not find process with TTBR0={:#x}", page_table_phys);
                    }
                });

                if terminated {
                    // Mark scheduler needs reschedule
                    crate::task::scheduler::set_need_resched();

                    // Switch scheduler to idle thread
                    crate::task::scheduler::switch_to_idle();

                    // Modify exception frame to return to idle loop
                    // The idle loop runs in EL1 and will handle rescheduling
                    frame_ref.elr = crate::arch_impl::aarch64::idle_loop_arm64 as *const () as u64;
                    frame_ref.spsr = 0x3c5; // EL1h, interrupts enabled

                    // Return to idle loop via ERET
                    return;
                }
            }

            // From kernel or couldn't terminate - hang
            loop { unsafe { core::arch::asm!("wfi"); } }
        }

        exception_class::INSTRUCTION_ABORT_LOWER | exception_class::INSTRUCTION_ABORT_SAME => {
            let frame = unsafe { &*frame };
            crate::serial_println!("[exception] Instruction abort at address {:#x}", far);
            crate::serial_println!("  ELR: {:#x}, ESR: {:#x}", frame.elr, esr);
            // For now, hang
            loop { unsafe { core::arch::asm!("wfi"); } }
        }

        exception_class::BRK_AARCH64 => {
            let frame = unsafe { &mut *frame };
            let imm = iss & 0xFFFF;
            crate::serial_println!("[exception] Breakpoint (BRK #{}) at {:#x}", imm, frame.elr);
            // Skip the BRK instruction
            frame.elr += 4;
        }

        _ => {
            let frame = unsafe { &*frame };
            crate::serial_println!("[exception] Unhandled sync exception");
            crate::serial_println!("  EC: {:#x}, ISS: {:#x}", ec, iss);
            crate::serial_println!("  ELR: {:#x}, FAR: {:#x}", frame.elr, far);
            // Hang
            loop { unsafe { core::arch::asm!("wfi"); } }
        }
    }
}

/// Syscall numbers (Linux/Breenix ABI compatible)
mod syscall_nums {
    // Core syscalls
    pub const EXIT: u64 = 0;
    pub const WRITE: u64 = 1;
    pub const READ: u64 = 2;
    pub const YIELD: u64 = 3;        // Breenix: sched_yield
    pub const GET_TIME: u64 = 4;     // Breenix: get_time (deprecated)
    pub const CLOSE: u64 = 6;        // Breenix: close
    pub const BRK: u64 = 12;         // Linux: brk

    // Process syscalls
    pub const GETPID: u64 = 39;
    pub const GETTID: u64 = 186;
    pub const CLOCK_GETTIME: u64 = 228;
}

/// Handle a syscall from userspace (or kernel for testing)
///
/// Uses the SyscallFrame trait to extract arguments in an arch-agnostic way.
/// ARM64-native implementation handles syscalls directly.
fn handle_syscall(frame: &mut Aarch64ExceptionFrame) {
    let syscall_num = frame.syscall_number();
    let arg1 = frame.arg1();
    let arg2 = frame.arg2();
    let arg3 = frame.arg3();

    let result = match syscall_num {
        syscall_nums::EXIT => {
            let exit_code = arg1 as i32;
            crate::serial_println!("[syscall] exit({})", exit_code);
            crate::serial_println!();
            crate::serial_println!("========================================");
            crate::serial_println!("  Userspace Test Complete!");
            crate::serial_println!("  Exit code: {}", exit_code);
            crate::serial_println!("========================================");
            crate::serial_println!();

            // For now, just halt - real implementation would terminate the process
            loop {
                unsafe { core::arch::asm!("wfi"); }
            }
        }

        syscall_nums::WRITE => {
            sys_write(arg1, arg2, arg3)
        }

        syscall_nums::READ => {
            // For now, read is not implemented
            SyscallResult::Err(38) // ENOSYS
        }

        syscall_nums::YIELD => {
            // Yield does nothing for single-process kernel
            SyscallResult::Ok(0)
        }

        syscall_nums::GET_TIME => {
            // Legacy get_time syscall - return milliseconds
            let ms = crate::time::get_monotonic_time();
            SyscallResult::Ok(ms)
        }

        syscall_nums::CLOSE => {
            // Close syscall - no file descriptors yet, just succeed
            SyscallResult::Ok(0)
        }

        syscall_nums::BRK => {
            // brk syscall - memory management
            // For now, return success with same address (no-op)
            SyscallResult::Ok(arg1)
        }

        syscall_nums::GETPID => {
            // Return a fixed PID for now (1 = init)
            SyscallResult::Ok(1)
        }

        syscall_nums::GETTID => {
            // Return a fixed TID for now (1 = main thread)
            SyscallResult::Ok(1)
        }

        syscall_nums::CLOCK_GETTIME => {
            sys_clock_gettime(arg1 as u32, arg2 as *mut Timespec)
        }

        _ => {
            crate::serial_println!("[syscall] ENOSYS for syscall {}", syscall_num);
            SyscallResult::Err(38) // ENOSYS
        }
    };

    // Convert SyscallResult to i64 return value
    let return_value: i64 = match result {
        SyscallResult::Ok(val) => val as i64,
        SyscallResult::Err(errno) => -(errno as i64),
    };

    // Set return value (negative values indicate errors in Linux convention)
    frame.set_return_value(return_value as u64);
}

/// Timespec structure for clock_gettime
#[repr(C)]
#[derive(Copy, Clone)]
pub struct Timespec {
    pub tv_sec: i64,
    pub tv_nsec: i64,
}

/// ARM64 sys_write implementation
fn sys_write(fd: u64, buf: u64, count: u64) -> SyscallResult {
    // Only support stdout (1) and stderr (2) for now
    if fd != 1 && fd != 2 {
        return SyscallResult::Err(9); // EBADF
    }

    // Validate buffer pointer (basic check)
    if buf == 0 {
        return SyscallResult::Err(14); // EFAULT
    }

    // Write each byte to serial
    for i in 0..count {
        let byte = unsafe { *((buf + i) as *const u8) };
        crate::serial_print!("{}", byte as char);
    }

    SyscallResult::Ok(count)
}

/// ARM64 sys_clock_gettime implementation
fn sys_clock_gettime(clock_id: u32, user_timespec_ptr: *mut Timespec) -> SyscallResult {
    // Validate pointer
    if user_timespec_ptr.is_null() {
        return SyscallResult::Err(14); // EFAULT
    }

    // Get time from the arch-agnostic time module
    let (tv_sec, tv_nsec) = match clock_id {
        0 => { // CLOCK_REALTIME
            crate::time::get_real_time_ns()
        }
        1 => { // CLOCK_MONOTONIC
            let (secs, nanos) = crate::time::get_monotonic_time_ns();
            (secs as i64, nanos as i64)
        }
        _ => {
            return SyscallResult::Err(22); // EINVAL
        }
    };

    // Write to userspace
    unsafe {
        (*user_timespec_ptr).tv_sec = tv_sec;
        (*user_timespec_ptr).tv_nsec = tv_nsec;
    }

    SyscallResult::Ok(0)
}

/// PL011 UART IRQ number (SPI 1, which is IRQ 33)
const UART0_IRQ: u32 = 33;

/// Raw serial write - write a string without locks, for use in interrupt handlers
#[inline(always)]
fn raw_serial_str(s: &[u8]) {
    crate::serial_aarch64::raw_serial_str(s);
}

/// Handle IRQ interrupts
///
/// Called from assembly after saving registers.
/// This is the main IRQ dispatch point for ARM64.
#[no_mangle]
pub extern "C" fn handle_irq() {
    // Acknowledge the interrupt from GIC
    if let Some(irq_id) = gic::acknowledge_irq() {
        // Handle the interrupt based on ID
        match irq_id {
            // Virtual timer interrupt (PPI 27)
            // This is the scheduling timer - calls into scheduler
            crate::arch_impl::aarch64::timer_interrupt::TIMER_IRQ => {
                // Call the timer interrupt handler which handles:
                // - Re-arming the timer
                // - Updating global time
                // - Decrementing time quantum
                // - Setting need_resched flag
                crate::arch_impl::aarch64::timer_interrupt::timer_interrupt_handler();
            }

            // UART0 receive interrupt (SPI 1 = IRQ 33)
            UART0_IRQ => {
                handle_uart_interrupt();
            }

            // SGIs (0-15) - Inter-processor interrupts
            0..=15 => {}

            // PPIs (16-31) - Private peripheral interrupts (excluding timer)
            16..=31 => {}

            // SPIs (32-1019) - Shared peripheral interrupts
            // Note: No logging here - interrupt handlers must be < 1000 cycles
            32..=1019 => {
                // VirtIO input (keyboard) interrupt dispatch
                if let Some(input_irq) = crate::drivers::virtio::input_mmio::get_irq() {
                    if irq_id == input_irq {
                        crate::drivers::virtio::input_mmio::handle_interrupt();
                    }
                }
                // VirtIO network interrupt dispatch
                if let Some(net_irq) = crate::drivers::virtio::net_mmio::get_irq() {
                    if irq_id == net_irq {
                        crate::drivers::virtio::net_mmio::handle_interrupt();
                    }
                }
            }

            // Should not happen - GIC filters invalid IDs (1020+)
            _ => {}
        }

        // Signal end of interrupt
        gic::end_of_interrupt(irq_id);

        // Process pending softirqs (deferred work from interrupt handlers)
        // This must happen after EOI but before rescheduling, while still
        // in the IRQ exit path. Network RX processing runs here.
        crate::task::softirqd::do_softirq();

        // Check if we need to reschedule after handling the interrupt
        // This is the ARM64 equivalent of x86's check_need_resched_and_switch
        check_need_resched_on_irq_exit();
    }
}

/// Check if rescheduling is needed and perform context switch if necessary
///
/// This is called at the end of IRQ handling, before returning via ERET.
/// It checks the need_resched flag and performs a context switch if needed.
///
/// Note: This is a simplified version that only handles the scheduling decision.
/// The actual context switch happens when the exception handler returns and
/// the assembly code uses the modified exception frame.
fn check_need_resched_on_irq_exit() {
    // Check if per-CPU data is initialized
    if !crate::per_cpu_aarch64::is_initialized() {
        return;
    }

    // Check if we're still in interrupt context (nested IRQs)
    // Note: Timer interrupt already decremented HARDIRQ count before we get here
    if crate::per_cpu_aarch64::in_interrupt() {
        return;
    }

    // Check if rescheduling is needed (don't clear yet - context_switch does that)
    if !crate::task::scheduler::is_need_resched() {
        return;
    }

    // The actual context switch will be performed by check_need_resched_and_switch_arm64
    // which is called from the exception return path with access to the exception frame.
    // Here we just signal that a reschedule is pending.
    //
    // The flow is:
    // 1. Timer IRQ fires -> timer_interrupt_handler() sets need_resched
    // 2. IRQ handler returns
    // 3. Assembly exception return path calls check_need_resched_and_switch_arm64
    // 4. Context switch happens if needed
    // 5. ERET returns to new thread
}

/// Handle UART receive interrupt
///
/// Read all available bytes from the UART and push to stdin buffer.
/// Echo is handled by the consumer (kernel shell or userspace tty driver).
fn handle_uart_interrupt() {
    use crate::serial_aarch64;

    // Read all available bytes from the UART FIFO
    while let Some(byte) = serial_aarch64::get_received_byte() {
        // Push to stdin buffer for kernel shell or userspace read() syscall
        // This wakes any blocked readers waiting for input
        crate::ipc::stdin::push_byte_from_irq(byte);
    }

    // Clear the interrupt
    serial_aarch64::clear_rx_interrupt();
}

/// Get exception class name for debugging
#[allow(dead_code)]
fn exception_class_name(ec: u32) -> &'static str {
    match ec {
        exception_class::UNKNOWN => "Unknown",
        exception_class::SVC_AARCH64 => "SVC (syscall)",
        exception_class::INSTRUCTION_ABORT_LOWER => "Instruction abort (lower EL)",
        exception_class::INSTRUCTION_ABORT_SAME => "Instruction abort (same EL)",
        exception_class::DATA_ABORT_LOWER => "Data abort (lower EL)",
        exception_class::DATA_ABORT_SAME => "Data abort (same EL)",
        exception_class::SP_ALIGNMENT => "SP alignment fault",
        exception_class::BRK_AARCH64 => "BRK (breakpoint)",
        _ => "Other",
    }
}

/// Handle CoW (Copy-on-Write) page fault for ARM64
///
/// Returns true if the fault was handled (page was copied or made writable)
/// Returns false if this wasn't a CoW fault or couldn't be handled
fn handle_cow_fault_arm64(far: u64, iss: u32) -> bool {
    use crate::memory::arch_stub::{VirtAddr, Page, Size4KiB};
    use crate::memory::cow_stats;
    use crate::memory::frame_allocator::allocate_frame;
    use crate::memory::frame_metadata::{frame_decref, frame_is_shared};
    use crate::memory::process_memory::{is_cow_page, make_private_flags};

    // Check if this is a CoW fault:
    // - WnR bit (bit 6) = 1 (caused by write)
    // - DFSC (bits 5:0) = 0x0D/0x0E/0x0F (Permission fault at level 1/2/3)
    let is_write = (iss >> 6) & 1 == 1;
    let dfsc = iss & 0x3F;
    let is_permission_fault = dfsc == 0x0D || dfsc == 0x0E || dfsc == 0x0F;

    if !is_write || !is_permission_fault {
        return false;
    }

    // Track CoW fault count
    cow_stats::TOTAL_FAULTS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);

    let faulting_addr = VirtAddr::new(far);

    // Get current TTBR0 (user page table base)
    let ttbr0: u64;
    unsafe {
        core::arch::asm!("mrs {}, ttbr0_el1", out(reg) ttbr0, options(nomem, nostack));
    }

    // Mask off ASID to get physical address
    let page_table_phys = ttbr0 & !0xFFFF_0000_0000_0FFF;

    crate::serial_println!(
        "[COW ARM64] fault at {:#x}, ttbr0={:#x}, pt_phys={:#x}",
        far,
        ttbr0,
        page_table_phys
    );

    // Try to acquire process manager lock
    match crate::process::try_manager() {
        Some(mut guard) => {
            cow_stats::MANAGER_PATH.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            let pm = match guard.as_mut() {
                Some(pm) => pm,
                None => return false,
            };

            // Find process by page table
            let (_pid, process) = match pm.find_process_by_cr3_mut(page_table_phys) {
                Some(p) => p,
                None => {
                    crate::serial_println!("[COW] No process found for TTBR0");
                    return false;
                }
            };

            let page_table = match &mut process.page_table {
                Some(pt) => pt,
                None => return false,
            };

            let page: Page<Size4KiB> = Page::containing_address(faulting_addr);

            // Get current page info
            let (old_frame, old_flags) = match page_table.get_page_info(page) {
                Some(info) => info,
                None => {
                    crate::serial_println!("[COW] No page info for {:#x}", far);
                    return false;
                }
            };

            // Check if this is a CoW page
            if !is_cow_page(old_flags) {
                crate::serial_println!("[COW] Not a CoW page");
                return false;
            }

            crate::serial_println!(
                "[COW] Handling page {:#x}, frame={:#x}, shared={}",
                far,
                old_frame.start_address().as_u64(),
                frame_is_shared(old_frame)
            );

            // If we're the sole owner, just make it writable
            if !frame_is_shared(old_frame) {
                let new_flags = make_private_flags(old_flags);
                if page_table.update_page_flags(page, new_flags).is_err() {
                    return false;
                }
                // Flush TLB
                unsafe {
                    let va_for_tlbi = faulting_addr.as_u64() >> 12;
                    core::arch::asm!(
                        "dsb ishst",
                        "tlbi vale1is, {0}",
                        "dsb ish",
                        "isb",
                        in(reg) va_for_tlbi,
                        options(nostack)
                    );
                }
                cow_stats::SOLE_OWNER_OPT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                crate::serial_println!("[COW] Made sole-owner page writable");
                return true;
            }

            // Need to copy the page
            let new_frame = match allocate_frame() {
                Some(f) => f,
                None => {
                    crate::serial_println!("[COW] Failed to allocate frame");
                    return false;
                }
            };

            // Copy page contents via HHDM
            let hhdm_base = crate::arch_impl::aarch64::constants::HHDM_BASE;
            let src = (hhdm_base + old_frame.start_address().as_u64()) as *const u8;
            let dst = (hhdm_base + new_frame.start_address().as_u64()) as *mut u8;

            unsafe {
                core::ptr::copy_nonoverlapping(src, dst, 4096);
            }

            // Unmap old page and map new one with write permissions
            let new_flags = make_private_flags(old_flags);
            if page_table.unmap_page(page).is_err() {
                crate::serial_println!("[COW] Failed to unmap old page");
                return false;
            }
            if page_table.map_page(page, new_frame, new_flags).is_err() {
                crate::serial_println!("[COW] Failed to map new page");
                return false;
            }

            // Decrement reference count on old frame
            frame_decref(old_frame);

            // Flush TLB
            unsafe {
                let va_for_tlbi = faulting_addr.as_u64() >> 12;
                core::arch::asm!(
                    "dsb ishst",
                    "tlbi vale1is, {0}",
                    "dsb ish",
                    "isb",
                    in(reg) va_for_tlbi,
                    options(nostack)
                );
            }

            cow_stats::PAGES_COPIED.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            crate::serial_println!(
                "[COW] Copied page from {:#x} to {:#x}",
                old_frame.start_address().as_u64(),
                new_frame.start_address().as_u64()
            );

            true
        }
        None => {
            cow_stats::DIRECT_PATH.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            crate::serial_println!("[COW] Manager lock held, cannot handle");
            false
        }
    }
}
