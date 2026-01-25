//! ARM64 exception handlers.
//!
//! These handlers are called from the assembly exception vector table.
//! They process synchronous exceptions (syscalls, page faults, etc.) and IRQs.

#![allow(dead_code)]

use crate::arch_impl::aarch64::gic;
use crate::arch_impl::aarch64::exception_frame::Aarch64ExceptionFrame;
use crate::arch_impl::traits::SyscallFrame;

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
            let frame = unsafe { &mut *frame };
            handle_syscall(frame);
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

/// Handle a syscall from userspace (or kernel for testing)
///
/// Uses the SyscallFrame trait to extract arguments in an arch-agnostic way.
fn handle_syscall(frame: &mut Aarch64ExceptionFrame) {
    let syscall_num = frame.syscall_number();
    let arg1 = frame.arg1();
    let arg2 = frame.arg2();
    let arg3 = frame.arg3();
    let _arg4 = frame.arg4();
    let _arg5 = frame.arg5();
    let _arg6 = frame.arg6();

    // For early boot testing, handle a few basic syscalls directly
    // This avoids pulling in the full syscall infrastructure which has x86_64 dependencies
    let result: i64 = match syscall_num {
        // Exit (syscall 0)
        0 => {
            let exit_code = arg1 as i32;
            crate::serial_println!("[syscall] exit({})", exit_code);
            crate::serial_println!();
            crate::serial_println!("========================================");
            crate::serial_println!("  Userspace Test Complete!");
            crate::serial_println!("  Exit code: {}", exit_code);
            crate::serial_println!("========================================");
            crate::serial_println!();

            // For now, just halt - real implementation would terminate the process
            // and schedule another task
            loop {
                unsafe { core::arch::asm!("wfi"); }
            }
        }

        // Write (syscall 1) - write to fd 1 (stdout) or 2 (stderr)
        1 => {
            let fd = arg1;
            let buf = arg2 as *const u8;
            let count = arg3 as usize;

            if fd == 1 || fd == 2 {
                // Write to serial console
                for i in 0..count {
                    let byte = unsafe { *buf.add(i) };
                    crate::serial_aarch64::write_byte(byte);
                }
                count as i64
            } else {
                -9i64 // EBADF
            }
        }

        // GetPid (syscall 39) - return a dummy PID
        39 => {
            1i64 // Return PID 1 for init
        }

        // GetTid (syscall 186) - return a dummy TID
        186 => {
            1i64 // Return TID 1
        }

        // ClockGetTime (syscall 228)
        228 => {
            let clock_id = arg1 as u32;
            let timespec_ptr = arg2 as *mut [u64; 2];

            if timespec_ptr.is_null() {
                -14i64 // EFAULT
            } else if clock_id > 1 {
                -22i64 // EINVAL
            } else {
                // Use the timer to get monotonic time
                if let Some((secs, nanos)) = crate::arch_impl::aarch64::timer::monotonic_time() {
                    unsafe {
                        (*timespec_ptr)[0] = secs;
                        (*timespec_ptr)[1] = nanos;
                    }
                    0i64
                } else {
                    -22i64 // EINVAL - timer not calibrated
                }
            }
        }

        // Unknown syscall
        _ => {
            crate::serial_println!("[syscall] unimplemented syscall {} (args: {:#x}, {:#x}, {:#x})",
                syscall_num, arg1, arg2, arg3);
            -38i64 // ENOSYS
        }
    };

    // Set return value (negative values indicate errors in Linux convention)
    frame.set_return_value(result as u64);
}

/// PL011 UART IRQ number (SPI 1, which is IRQ 33)
const UART0_IRQ: u32 = 33;

/// Raw serial write - no locks, for use in interrupt handlers
#[inline(always)]
fn raw_serial_char(c: u8) {
    const PL011_DR: *mut u32 = 0x0900_0000 as *mut u32;
    unsafe { core::ptr::write_volatile(PL011_DR, c as u32); }
}

/// Handle IRQ interrupts
///
/// Called from assembly after saving registers
#[no_mangle]
pub extern "C" fn handle_irq() {
    // Debug: show we entered IRQ handler
    raw_serial_char(b'I');

    // Acknowledge the interrupt from GIC
    if let Some(irq_id) = gic::acknowledge_irq() {
        // Debug: show IRQ ID (as hex digit if < 16, else 'X')
        raw_serial_char(b':');
        if irq_id < 10 {
            raw_serial_char(b'0' + irq_id as u8);
        } else if irq_id < 16 {
            raw_serial_char(b'a' + (irq_id - 10) as u8);
        } else if irq_id < 100 {
            raw_serial_char(b'0' + (irq_id / 10) as u8);
            raw_serial_char(b'0' + (irq_id % 10) as u8);
        } else {
            raw_serial_char(b'X');
        }
        raw_serial_char(b' ');
        // Handle the interrupt based on ID
        match irq_id {
            // Virtual timer interrupt (PPI 27, but shows as 27 in IAR)
            27 => {
                // Timer interrupt - clear it without logging to avoid noise
                crate::arch_impl::aarch64::timer::disarm_timer();
            }

            // UART0 receive interrupt (SPI 1 = IRQ 33)
            UART0_IRQ => {
                handle_uart_interrupt();
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

/// Handle UART receive interrupt
///
/// Read all available bytes from the UART and route them to the terminal.
fn handle_uart_interrupt() {
    use crate::serial_aarch64;
    use crate::graphics::terminal_manager;

    // Debug marker: 'U' for UART interrupt entry
    raw_serial_char(b'U');

    // Read all available bytes from the UART FIFO
    while let Some(byte) = serial_aarch64::get_received_byte() {
        // Debug: echo the byte we received
        raw_serial_char(b'[');
        raw_serial_char(byte);
        raw_serial_char(b']');

        // Handle special keys
        let c = match byte {
            // Backspace
            0x7F | 0x08 => '\x08',
            // Enter
            0x0D => '\n',
            // Tab
            0x09 => '\t',
            // Escape sequences start with 0x1B - for now, ignore
            0x1B => continue,
            // Regular ASCII
            b if b >= 0x20 && b < 0x7F => byte as char,
            // Control characters (Ctrl+C = 0x03, Ctrl+D = 0x04, etc.)
            b if b < 0x20 => byte as char,
            _ => continue,
        };

        // Write to the shell terminal (handles locking internally)
        terminal_manager::write_char_to_shell(c);
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
