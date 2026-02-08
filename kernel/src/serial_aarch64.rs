//! ARM64 serial I/O using PL011 UART.
//!
//! Provides both output and input for ARM64 via PL011 UART at MMIO address
//! 0x0900_0000 (QEMU virt machine). Input is interrupt-driven using IRQ 33.

#![cfg(target_arch = "aarch64")]

use core::fmt;
use core::sync::atomic::{AtomicBool, Ordering};
use spin::Mutex;

// =============================================================================
// PL011 UART Register Map
// =============================================================================

/// PL011 UART base physical address for QEMU virt machine.
const PL011_BASE_PHYS: usize = 0x0900_0000;

/// PL011 Register offsets - complete register map for UART configuration
#[allow(dead_code)]
mod reg {
    /// Data Register (read/write)
    pub const DR: usize = 0x00;
    /// Receive Status Register / Error Clear Register
    pub const RSRECR: usize = 0x04;
    /// Flag Register (read-only)
    pub const FR: usize = 0x18;
    /// Integer Baud Rate Register
    pub const IBRD: usize = 0x24;
    /// Fractional Baud Rate Register
    pub const FBRD: usize = 0x28;
    /// Line Control Register
    pub const LCR_H: usize = 0x2C;
    /// Control Register
    pub const CR: usize = 0x30;
    /// Interrupt FIFO Level Select Register
    pub const IFLS: usize = 0x34;
    /// Interrupt Mask Set/Clear Register
    pub const IMSC: usize = 0x38;
    /// Raw Interrupt Status Register
    pub const RIS: usize = 0x3C;
    /// Masked Interrupt Status Register
    pub const MIS: usize = 0x40;
    /// Interrupt Clear Register
    pub const ICR: usize = 0x44;
}

/// Flag Register bits
mod flag {
    /// Receive FIFO empty
    pub const RXFE: u32 = 1 << 4;
    /// Transmit FIFO full
    pub const TXFF: u32 = 1 << 5;
}

/// Interrupt bits
mod int {
    /// Receive interrupt
    pub const RX: u32 = 1 << 4;
    /// Receive timeout interrupt
    pub const RT: u32 = 1 << 6;
}

/// Control Register bits
mod cr {
    /// UART enable
    pub const UARTEN: u32 = 1 << 0;
    /// Transmit enable
    pub const TXE: u32 = 1 << 8;
    /// Receive enable
    pub const RXE: u32 = 1 << 9;
}

// =============================================================================
// Register Access Helpers
// =============================================================================

#[inline]
fn read_reg(offset: usize) -> u32 {
    unsafe {
        let base = crate::memory::physical_memory_offset().as_u64() as usize;
        let addr = (base + PL011_BASE_PHYS + offset) as *const u32;
        core::ptr::read_volatile(addr)
    }
}

#[inline]
fn write_reg(offset: usize, value: u32) {
    unsafe {
        let base = crate::memory::physical_memory_offset().as_u64() as usize;
        let addr = (base + PL011_BASE_PHYS + offset) as *mut u32;
        core::ptr::write_volatile(addr, value);
    }
}

// =============================================================================
// Serial Port Implementation
// =============================================================================

/// PL011 UART serial port.
pub struct SerialPort;

/// Whether serial is initialized
static SERIAL_INITIALIZED: AtomicBool = AtomicBool::new(false);

impl SerialPort {
    pub const fn new(_base: u16) -> Self {
        SerialPort
    }

    pub fn init(&mut self) {
        if SERIAL_INITIALIZED.load(Ordering::Relaxed) {
            return;
        }

        // QEMU already has UART working for TX.
        // Just ensure UARTEN is set for RX to work.
        // Don't do a full reinit to avoid disrupting QEMU's setup.
        let cr = read_reg(reg::CR);
        write_reg(reg::CR, cr | cr::UARTEN | cr::TXE | cr::RXE);

        SERIAL_INITIALIZED.store(true, Ordering::Release);
    }

    /// Send a single byte
    pub fn send(&mut self, byte: u8) {
        // Wait until TX FIFO is not full
        while (read_reg(reg::FR) & flag::TXFF) != 0 {
            core::hint::spin_loop();
        }
        write_reg(reg::DR, byte as u32);
    }

    /// Try to receive a byte (non-blocking)
    ///
    /// Returns None if the receive FIFO is empty.
    pub fn try_receive(&self) -> Option<u8> {
        if (read_reg(reg::FR) & flag::RXFE) != 0 {
            None
        } else {
            Some((read_reg(reg::DR) & 0xFF) as u8)
        }
    }

    /// Check if there is data available to read
    pub fn is_data_available(&self) -> bool {
        (read_reg(reg::FR) & flag::RXFE) == 0
    }
}

impl fmt::Write for SerialPort {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for byte in s.bytes() {
            self.send(byte);
        }
        Ok(())
    }
}

// =============================================================================
// Global Serial Ports
// =============================================================================

pub static SERIAL1: Mutex<SerialPort> = Mutex::new(SerialPort::new(0));
pub static SERIAL2: Mutex<SerialPort> = Mutex::new(SerialPort::new(0));

pub fn init_serial() {
    SERIAL1.lock().init();
}

/// Enable receive interrupts on the UART
///
/// Must be called after GIC is initialized to properly route the interrupt.
pub fn enable_rx_interrupt() {
    // Enable RX and RX timeout interrupts
    let old_imsc = read_reg(reg::IMSC);
    let new_imsc = old_imsc | int::RX | int::RT;
    write_reg(reg::IMSC, new_imsc);

    // Debug: verify the write
    let verify = read_reg(reg::IMSC);
    crate::serial_println!("[uart] IMSC: {:#x} -> {:#x} (verify: {:#x})", old_imsc, new_imsc, verify);

    // Also show the flag register and interrupt status
    let fr = read_reg(reg::FR);
    let ris = read_reg(reg::RIS);
    crate::serial_println!("[uart] FR={:#x} (RXFE={}), RIS={:#x}", fr, (fr >> 4) & 1, ris);
}

/// Disable receive interrupts
pub fn disable_rx_interrupt() {
    let imsc = read_reg(reg::IMSC);
    write_reg(reg::IMSC, imsc & !(int::RX | int::RT));
}

/// Clear receive interrupt
pub fn clear_rx_interrupt() {
    write_reg(reg::ICR, int::RX | int::RT);
}

/// Get any pending received byte and clear the interrupt
///
/// Returns None if no data available.
pub fn get_received_byte() -> Option<u8> {
    if (read_reg(reg::FR) & flag::RXFE) != 0 {
        None
    } else {
        Some((read_reg(reg::DR) & 0xFF) as u8)
    }
}

/// Write a single byte to serial output.
///
/// Disables interrupts before acquiring the lock to prevent deadlock:
/// without this, a timer IRQ could fire while holding SERIAL1, and
/// any code on another CPU holding SCHEDULER that tries to log would
/// create a SERIAL1 → SCHEDULER / SCHEDULER → SERIAL1 deadlock.
pub fn write_byte(byte: u8) {
    let daif_before: u64;
    unsafe {
        core::arch::asm!("mrs {}, DAIF", out(reg) daif_before, options(nomem, nostack));
        core::arch::asm!("msr DAIFSet, #0x3", options(nomem, nostack));
    }
    SERIAL1.lock().send(byte);
    unsafe {
        core::arch::asm!("msr DAIF, {}", in(reg) daif_before, options(nomem, nostack));
    }
}

/// Writer that tees output to both UART and the log capture ring buffer.
struct TeeWriter<'a>(&'a mut SerialPort);

impl fmt::Write for TeeWriter<'_> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for byte in s.bytes() {
            self.0.send(byte);
            crate::graphics::log_capture::capture_byte(byte);
        }
        Ok(())
    }
}

#[doc(hidden)]
pub fn _print(args: fmt::Arguments) {
    use core::fmt::Write;

    // CRITICAL: Disable interrupts during serial output to prevent deadlock.
    // On ARM64, unlike x86_64, there's only one serial port (SERIAL1).
    // Both serial_println! and log_serial_println! use the same lock.
    // If a timer interrupt fires while holding the lock and something in
    // the timer path tries to log, we get a deadlock.
    //
    // The interrupt disable is done via DAIF (Debug/Abort/IRQ/FIQ mask) bits.
    let daif_before: u64;
    unsafe {
        // Read current interrupt state
        core::arch::asm!("mrs {}, DAIF", out(reg) daif_before, options(nomem, nostack));
        // Mask IRQ (bit 7) and FIQ (bit 6)
        core::arch::asm!("msr DAIFSet, #0x3", options(nomem, nostack));
    }

    let mut serial = SERIAL1.lock();
    // Tee: write to both UART and log capture buffer
    let mut tee = TeeWriter(&mut *serial);
    let _ = write!(tee, "{}", args);
    drop(serial);

    // Restore previous interrupt state
    unsafe {
        core::arch::asm!("msr DAIF, {}", in(reg) daif_before, options(nomem, nostack));
    }
}

/// Try to print without blocking - returns Err if lock is held
pub fn try_print(args: fmt::Arguments) -> Result<(), ()> {
    use core::fmt::Write;

    match SERIAL1.try_lock() {
        Some(mut serial) => {
            serial.write_fmt(args).map_err(|_| ())?;
            Ok(())
        }
        None => Err(()), // Lock is held
    }
}

/// Emergency print for panics - uses direct port I/O without locking
#[allow(dead_code)]
pub fn emergency_print(args: fmt::Arguments) -> Result<(), ()> {
    use core::fmt::Write;

    struct EmergencySerial;

    impl fmt::Write for EmergencySerial {
        fn write_str(&mut self, s: &str) -> fmt::Result {
            for byte in s.bytes() {
                // Wait for TX FIFO
                while (read_reg(reg::FR) & flag::TXFF) != 0 {
                    core::hint::spin_loop();
                }
                write_reg(reg::DR, byte as u32);
            }
            Ok(())
        }
    }

    let mut emergency = EmergencySerial;
    emergency.write_fmt(args).map_err(|_| ())?;
    Ok(())
}

/// Log print function for the log_serial_print macro
#[doc(hidden)]
pub fn _log_print(args: fmt::Arguments) {
    _print(args);
}

// =============================================================================
// Lock-Free Debug Output for Critical Paths
// =============================================================================

/// Raw serial debug output - single character, no locks, no allocations.
/// Safe to call from any context including interrupt handlers and syscalls.
///
/// This is the ONLY acceptable way to add debug markers to critical paths like:
/// - Context switch code
/// - Kernel thread entry
/// - Workqueue workers
/// - Interrupt handlers
/// - Syscall entry/exit
#[inline(always)]
pub fn raw_serial_char(c: u8) {
    const HHDM_BASE: u64 = 0xFFFF_0000_0000_0000;
    let addr = (HHDM_BASE + PL011_BASE_PHYS as u64) as *mut u32;
    unsafe { core::ptr::write_volatile(addr, c as u32); }
}

/// Raw serial debug output - write a string without locks or allocations.
/// Safe to call from any context including interrupt handlers and syscalls.
///
/// Use this for unique, descriptive debug markers that are easy to grep for:
/// - `raw_serial_str(b"[STDIN_READ]")` instead of `raw_serial_char(b'r')`
/// - `raw_serial_str(b"[VIRTIO_KEY]")` instead of `raw_serial_char(b'V')`
///
/// This helps identify markers in test output without ambiguity.
#[inline(always)]
pub fn raw_serial_str(s: &[u8]) {
    const HHDM_BASE: u64 = 0xFFFF_0000_0000_0000;
    let addr = (HHDM_BASE + PL011_BASE_PHYS as u64) as *mut u32;
    for &c in s {
        unsafe { core::ptr::write_volatile(addr, c as u32); }
    }
}

#[macro_export]
macro_rules! serial_print {
    ($($arg:tt)*) => {
        $crate::serial_aarch64::_print(format_args!($($arg)*));
    };
}

#[macro_export]
macro_rules! serial_println {
    () => ($crate::serial_print!("\n"));
    ($($arg:tt)*) => ($crate::serial_print!("{}\n", format_args!($($arg)*)));
}

#[macro_export]
macro_rules! log_serial_print {
    ($($arg:tt)*) => ($crate::serial::_log_print(format_args!($($arg)*)));
}

#[macro_export]
macro_rules! log_serial_println {
    () => ($crate::log_serial_print!("\n"));
    ($($arg:tt)*) => ($crate::log_serial_print!("{}\n", format_args!($($arg)*)));
}
