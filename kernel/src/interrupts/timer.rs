//! Timer interrupt handler following OS design best practices
//!
//! This handler is MINIMAL - it does the absolute minimum work required in interrupt context:
//! 1. Updates the global timer tick count (via crate::time::timer_interrupt)
//! 2. Decrements current thread's time quantum
//! 3. Sets need_resched flag if quantum expired
//! 4. Returns (EOI is sent in assembly just before IRETQ)
//!
//! DESIGN RATIONALE:
//! - No logging: prevents serial lock deadlock and keeps handler fast
//! - No diagnostics: no counters, no TSC timing, no periodic output
//! - No nested interrupt detection: if handler is fast enough, nesting won't occur
//! - All complex logic deferred to: bottom-half handlers, separate diagnostic threads,
//!   or userspace monitoring tools
//!
//! All context switching happens on the interrupt return path via irq_exit().

use crate::task::scheduler;

/// Time quantum in timer ticks (5ms per tick @ 200 Hz, 50ms quantum = 10 ticks)
const TIME_QUANTUM: u32 = 10;

/// Current thread's remaining time quantum
static mut CURRENT_QUANTUM: u32 = TIME_QUANTUM;

/// Timer interrupt handler - absolutely minimal work
///
/// @param from_userspace: 1 if interrupted userspace, 0 if interrupted kernel (unused)
#[no_mangle]
pub extern "C" fn timer_interrupt_handler(_from_userspace: u8) {
    // Enter hardware IRQ context (increments HARDIRQ count)
    crate::per_cpu::irq_enter();

    // Core time bookkeeping: increment TICKS counter (single atomic operation)
    crate::time::timer_interrupt();

    // Decrement current thread's quantum and check for reschedule
    unsafe {
        // Use raw pointer to avoid creating references to mutable static (Rust 2024 compatibility)
        let quantum_ptr = core::ptr::addr_of_mut!(CURRENT_QUANTUM);

        if *quantum_ptr > 0 {
            *quantum_ptr -= 1;
        }

        // Only reschedule when quantum expires - NOT every tick
        // Rescheduling on every tick prevents userspace from executing.
        if *quantum_ptr == 0 {
            scheduler::set_need_resched();
            *quantum_ptr = TIME_QUANTUM; // Reset for next thread
        }
    }

    // CRITICAL: EOI is sent by send_timer_eoi() called from timer_entry.asm
    // just before IRETQ, after all processing is complete. Sending EOI earlier
    // allows the PIC to queue another timer interrupt during context switch
    // processing, which fires immediately when IRETQ re-enables interrupts.

    // Exit hardware IRQ context (decrements HARDIRQ count and may trigger context switch)
    crate::per_cpu::irq_exit();
}

/// Reset the quantum counter (called when switching threads)
pub fn reset_quantum() {
    unsafe {
        // Use raw pointer to avoid creating reference to mutable static (Rust 2024 compatibility)
        let quantum_ptr = core::ptr::addr_of_mut!(CURRENT_QUANTUM);
        *quantum_ptr = TIME_QUANTUM;
    }
}

/// Debug function to log timer interrupt frame from userspace
/// Called from assembly when timer interrupt arrives from userspace
#[no_mangle]
pub extern "C" fn log_timer_frame_from_userspace(_frame: *const u64) {
    // Disabled to avoid serial lock contention during timer interrupts
    // Uncomment only for deep debugging sessions
    // unsafe {
    //     crate::serial_println!("Timer from userspace");
    // }
}

/// Debug function to dump IRET frame to serial port
/// Called from assembly just before IRETQ to userspace
#[no_mangle]
pub extern "C" fn dump_iret_frame_to_serial(_frame: *const u64) {
    // Disabled to avoid serial lock contention during timer interrupts
    // Uncomment only for deep debugging sessions
    // unsafe {
    //     if !_frame.is_null() {
    //         let rip = *_frame.offset(0);
    //         let cs = *_frame.offset(1);
    //         let rflags = *_frame.offset(2);
    //         let rsp = *_frame.offset(3);
    //         let ss = *_frame.offset(4);
    //         crate::serial_println!(
    //             "IRET: RIP={:#x} CS={:#x} RFLAGS={:#x} RSP={:#x} SS={:#x}",
    //             rip, cs, rflags, rsp, ss
    //         );
    //     }
    // }
}

/// Send End-Of-Interrupt for the timer
///
/// CRITICAL: This MUST be called just before IRETQ, after all timer interrupt
/// processing is complete. Calling EOI earlier allows the PIC to queue another
/// timer interrupt during processing, which fires immediately when IRETQ
/// re-enables interrupts.
///
/// Uses try_lock() to avoid deadlock if a nested timer interrupt fires while
/// the lock is held. If try_lock fails, sends EOI directly to the PIC hardware.
#[no_mangle]
pub extern "C" fn send_timer_eoi() {
    unsafe {
        // Try to acquire the PICS lock without blocking
        if let Some(mut pics) = super::PICS.try_lock() {
            // Lock acquired successfully, send EOI through the PICS abstraction
            pics.notify_end_of_interrupt(super::InterruptIndex::Timer.as_u8());
        } else {
            // Lock contention detected - use direct hardware access to avoid deadlock
            use x86_64::instructions::port::Port;

            let interrupt_id = super::InterruptIndex::Timer.as_u8();

            // Timer (IRQ0) is interrupt 32, which is on PIC1 (master)
            // PIC1 handles 32-39, PIC2 handles 40-47
            const PIC_1_OFFSET: u8 = 32;
            const PIC_2_OFFSET: u8 = 40;

            if interrupt_id >= PIC_1_OFFSET && interrupt_id < PIC_2_OFFSET + 8 {
                // If on PIC2 (slave), send EOI to slave first
                if interrupt_id >= PIC_2_OFFSET {
                    let mut pic2_cmd: Port<u8> = Port::new(0xA0);
                    pic2_cmd.write(0x20);
                }
                // Always send EOI to PIC1 (master) for all PIC interrupts
                let mut pic1_cmd: Port<u8> = Port::new(0x20);
                pic1_cmd.write(0x20);
            }
        }
    }
}
