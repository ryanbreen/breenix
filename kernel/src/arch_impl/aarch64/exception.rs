//! ARM64 exception handlers.
//!
//! These handlers are called from the assembly exception vector table.
//! They process synchronous exceptions (syscalls, page faults, etc.) and IRQs.

#![allow(dead_code)]

use crate::arch_impl::aarch64::gic;

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

/// Exception frame passed from assembly
/// Must match the layout in boot.S
#[repr(C)]
pub struct ExceptionFrame {
    pub x0: u64,
    pub x1: u64,
    pub x2: u64,
    pub x3: u64,
    pub x4: u64,
    pub x5: u64,
    pub x6: u64,
    pub x7: u64,
    pub x8: u64,
    pub x9: u64,
    pub x10: u64,
    pub x11: u64,
    pub x12: u64,
    pub x13: u64,
    pub x14: u64,
    pub x15: u64,
    pub x16: u64,
    pub x17: u64,
    pub x18: u64,
    pub x19: u64,
    pub x20: u64,
    pub x21: u64,
    pub x22: u64,
    pub x23: u64,
    pub x24: u64,
    pub x25: u64,
    pub x26: u64,
    pub x27: u64,
    pub x28: u64,
    pub x29: u64,  // Frame pointer
    pub x30: u64,  // Link register
    pub elr: u64,  // Exception Link Register (return address)
    pub spsr: u64, // Saved Program Status Register
}

/// Handle synchronous exceptions (syscalls, page faults, etc.)
///
/// Called from assembly with:
/// - x0 = pointer to ExceptionFrame
/// - x1 = ESR_EL1 (Exception Syndrome Register)
/// - x2 = FAR_EL1 (Fault Address Register)
#[no_mangle]
pub extern "C" fn handle_sync_exception(frame: *mut ExceptionFrame, esr: u64, far: u64) {
    let ec = ((esr >> 26) & 0x3F) as u32;  // Exception Class
    let iss = (esr & 0x1FFFFFF) as u32;    // Instruction Specific Syndrome

    match ec {
        exception_class::SVC_AARCH64 => {
            // Syscall - ISS contains the immediate value from SVC instruction
            // For Linux ABI: syscall number in X8, args in X0-X5, return in X0
            let frame = unsafe { &mut *frame };
            let syscall_num = frame.x8;
            crate::serial_println!("[exception] SVC syscall #{} (not yet implemented)", syscall_num);
            // For now, return -ENOSYS
            frame.x0 = (-38i64) as u64;  // -ENOSYS
        }

        exception_class::DATA_ABORT_LOWER | exception_class::DATA_ABORT_SAME => {
            let frame = unsafe { &*frame };
            crate::serial_println!("[exception] Data abort at address {:#x}", far);
            crate::serial_println!("  ELR: {:#x}, ESR: {:#x}", frame.elr, esr);
            crate::serial_println!("  ISS: {:#x} (WnR={}, DFSC={:#x})",
                iss, (iss >> 6) & 1, iss & 0x3F);
            // For now, hang
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

/// Handle IRQ interrupts
///
/// Called from assembly after saving registers
#[no_mangle]
pub extern "C" fn handle_irq() {
    // Acknowledge the interrupt from GIC
    if let Some(irq_id) = gic::acknowledge_irq() {
        // Handle the interrupt based on ID
        match irq_id {
            // Virtual timer interrupt (PPI 27, but shows as 27 in IAR)
            27 => {
                crate::serial_println!("[irq] Timer interrupt");
                // Clear the timer interrupt by disabling it
                // (real handler would reschedule)
                crate::arch_impl::aarch64::timer::disarm_timer();
            }

            // SGIs (0-15) - Inter-processor interrupts
            0..=15 => {
                crate::serial_println!("[irq] SGI {} received", irq_id);
            }

            // PPIs (16-31) - Private peripheral interrupts
            16..=31 => {
                crate::serial_println!("[irq] PPI {} received", irq_id);
            }

            // SPIs (32+) - Shared peripheral interrupts
            _ => {
                crate::serial_println!("[irq] SPI {} received", irq_id);
            }
        }

        // Signal end of interrupt
        gic::end_of_interrupt(irq_id);
    }
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
