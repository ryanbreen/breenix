//! ARM64 serial I/O using PL011 UART.
//!
//! Provides both output and input for ARM64 via PL011 UART at MMIO address
//! 0x0900_0000 (QEMU virt machine). Input is interrupt-driven using IRQ 33.

#![cfg(target_arch = "aarch64")]

use core::fmt;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use spin::Mutex;

/// Global UART virtual address for assembly-level diagnostics.
/// Set during serial init so boot.S exception vectors can output
/// characters without calling into Rust (used for pre-SP diagnostics).
#[no_mangle]
pub static DIAG_UART_VIRT: AtomicU64 = AtomicU64::new(0);

// =============================================================================
// PL011 UART Register Map
// =============================================================================

/// PL011 UART base physical address.
/// Reads from platform_config (defaults to QEMU virt 0x0900_0000).
#[inline]
fn pl011_base_phys() -> usize {
    crate::platform_config::uart_base_phys() as usize
}

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

/// 16550 Register offsets (byte-spaced MMIO)
#[allow(dead_code)]
mod reg16550 {
    /// Transmit Holding Register / Receive Buffer Register
    pub const THR: usize = 0x00;
    /// Interrupt Enable Register
    pub const IER: usize = 0x01;
    /// FIFO Control Register (write) / Interrupt ID (read)
    pub const FCR: usize = 0x02;
    /// Line Control Register
    pub const LCR: usize = 0x03;
    /// Modem Control Register
    pub const MCR: usize = 0x04;
    /// Line Status Register
    pub const LSR: usize = 0x05;
    /// LSR bit: Transmit Holding Register Empty
    pub const LSR_THRE: u8 = 1 << 5;
    /// LSR bit: Data Ready
    pub const LSR_DR: u8 = 1 << 0;
    /// LCR value: 8 data bits, 1 stop bit, no parity
    pub const LCR_8N1: u8 = 0x03;
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

/// Check if the UART is 16550 type
#[inline]
fn is_16550() -> bool {
    crate::platform_config::uart_type() == 1
}

/// Read a PL011 register (32-bit MMIO)
#[inline]
fn read_reg(offset: usize) -> u32 {
    unsafe {
        let base = crate::memory::physical_memory_offset().as_u64() as usize;
        let addr = (base + pl011_base_phys() + offset) as *const u32;
        core::ptr::read_volatile(addr)
    }
}

/// Write a PL011 register (32-bit MMIO)
#[inline]
fn write_reg(offset: usize, value: u32) {
    unsafe {
        let base = crate::memory::physical_memory_offset().as_u64() as usize;
        let addr = (base + pl011_base_phys() + offset) as *mut u32;
        core::ptr::write_volatile(addr, value);
    }
}

/// Read a 16550 register (8-bit MMIO)
#[inline]
fn read_reg8(offset: usize) -> u8 {
    unsafe {
        let base = crate::memory::physical_memory_offset().as_u64() as usize;
        let addr = (base + pl011_base_phys() + offset) as *const u8;
        core::ptr::read_volatile(addr)
    }
}

/// Write a 16550 register (8-bit MMIO)
#[inline]
fn write_reg8(offset: usize, value: u8) {
    unsafe {
        let base = crate::memory::physical_memory_offset().as_u64() as usize;
        let addr = (base + pl011_base_phys() + offset) as *mut u8;
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

        if is_16550() {
            // 16550 UART: initialize with 8N1, enable FIFO
            write_reg8(reg16550::LCR, reg16550::LCR_8N1); // 8 data bits, 1 stop, no parity
            write_reg8(reg16550::FCR, 0x07); // Enable FIFO, clear RX/TX
            write_reg8(reg16550::IER, 0x00); // Disable interrupts initially
            write_reg8(reg16550::MCR, 0x00); // No modem control
        } else {
            // PL011: QEMU already has UART working for TX.
            // Just ensure UARTEN is set for RX to work.
            let cr = read_reg(reg::CR);
            write_reg(reg::CR, cr | cr::UARTEN | cr::TXE | cr::RXE);
        }

        // ARM64 kernel records need severity for the console session policy.
        if log::set_logger(&KERNEL_LOGGER).is_ok() {
            log::set_max_level(log::LevelFilter::Info);
        }
        SERIAL_INITIALIZED.store(true, Ordering::Release);

        // Publish UART virtual address for assembly-level diagnostics
        DIAG_UART_VIRT.store(crate::platform_config::uart_virt(), Ordering::Release);
    }

    /// Send a single byte
    pub fn send(&mut self, byte: u8) {
        crate::serial_line::Line::new().char(byte);
    }

    /// Try to receive a byte (non-blocking)
    ///
    /// Returns None if the receive FIFO is empty.
    pub fn try_receive(&self) -> Option<u8> {
        if is_16550() {
            if read_reg8(reg16550::LSR) & reg16550::LSR_DR == 0 {
                None
            } else {
                Some(read_reg8(reg16550::THR))
            }
        } else {
            if (read_reg(reg::FR) & flag::RXFE) != 0 {
                None
            } else {
                Some((read_reg(reg::DR) & 0xFF) as u8)
            }
        }
    }

    /// Check if there is data available to read
    pub fn is_data_available(&self) -> bool {
        if is_16550() {
            read_reg8(reg16550::LSR) & reg16550::LSR_DR != 0
        } else {
            (read_reg(reg::FR) & flag::RXFE) == 0
        }
    }
}

impl fmt::Write for SerialPort {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        crate::serial_line::Line::new().text(s);
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
    if is_16550() {
        // 16550: enable RX data available interrupt
        let ier = read_reg8(reg16550::IER);
        write_reg8(reg16550::IER, ier | 0x01); // bit 0 = RX data available
        crate::serial_println!("[uart-16550] IER: {:#x} -> {:#x}", ier, ier | 0x01);
    } else {
        // PL011: enable RX and RX timeout interrupts
        let old_imsc = read_reg(reg::IMSC);
        let new_imsc = old_imsc | int::RX | int::RT;
        write_reg(reg::IMSC, new_imsc);

        let verify = read_reg(reg::IMSC);
        crate::serial_println!(
            "[uart] IMSC: {:#x} -> {:#x} (verify: {:#x})",
            old_imsc,
            new_imsc,
            verify
        );

        let fr = read_reg(reg::FR);
        let ris = read_reg(reg::RIS);
        crate::serial_println!(
            "[uart] FR={:#x} (RXFE={}), RIS={:#x}",
            fr,
            (fr >> 4) & 1,
            ris
        );
    }
}

/// Disable receive interrupts
pub fn disable_rx_interrupt() {
    if is_16550() {
        write_reg8(reg16550::IER, 0x00);
    } else {
        let imsc = read_reg(reg::IMSC);
        write_reg(reg::IMSC, imsc & !(int::RX | int::RT));
    }
}

/// Rearm a deferred receive without boot-time diagnostics or a serial lock.
pub(crate) fn resume_rx_interrupt() {
    if is_16550() {
        let ier = read_reg8(reg16550::IER);
        write_reg8(reg16550::IER, ier | 0x01);
    } else {
        let imsc = read_reg(reg::IMSC);
        write_reg(reg::IMSC, imsc | int::RX | int::RT);
    }
}

/// Acknowledge the receive interrupt once the FIFO has reported empty.
///
/// It does not read the receive register: a byte that arrived after the
/// empty report would be consumed and lost (#1803).
pub fn clear_rx_interrupt() {
    // The 16550's received-data and character-timeout interrupts are cleared
    // by the reads that drained the FIFO, so an empty 16550 FIFO needs no
    // acknowledgement.
    if !is_16550() {
        write_reg(reg::ICR, int::RX | int::RT);
    }
}

/// Get any pending received byte and clear the interrupt
///
/// Returns None if no data available.
pub fn get_received_byte() -> Option<u8> {
    if is_16550() {
        if read_reg8(reg16550::LSR) & reg16550::LSR_DR == 0 {
            None
        } else {
            Some(read_reg8(reg16550::THR))
        }
    } else {
        if (read_reg(reg::FR) & flag::RXFE) != 0 {
            None
        } else {
            Some((read_reg(reg::DR) & 0xFF) as u8)
        }
    }
}

struct KernelLogger;
static KERNEL_LOGGER: KernelLogger = KernelLogger;

impl log::Log for KernelLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= log::Level::Info
    }

    fn log(&self, record: &log::Record) {
        if self.enabled(record.metadata()) {
            use core::fmt::Write;
            let _ = writeln!(crate::serial_line::TeeLine::with_level(record.level()),
                "{:>5}: {}", record.level(), record.args());
        }
    }

    fn flush(&self) {}
}

pub use crate::serial_writer::Writer;

/// Write a single byte from `writer` to serial output.
pub fn write_byte(writer: Writer, byte: u8) {
    write_bytes_atomic(writer, core::slice::from_ref(&byte));
}

/// Write one caller-defined output unit from `writer` as a single record (see
/// `serial_line`).
pub fn write_bytes_atomic(writer: Writer, bytes: &[u8]) {
    crate::serial_line::write_bytes(writer, bytes);
}

#[doc(hidden)]
pub fn _print(args: fmt::Arguments) {
    use core::fmt::Write;
    let _ = crate::serial_line::TeeLine::new().write_fmt(args);
}

/// A kernel diagnostic record (`diag_println!`): serial and the kernel log, at DEBUG,
/// which the screen console does not draw (`docs/console/contract.md` section 1.3).
#[doc(hidden)]
pub fn _diag_print(args: fmt::Arguments) {
    use core::fmt::Write;
    let _ = crate::serial_line::TeeLine::with_level(log::Level::Debug).write_fmt(args);
}

/// Print without risk of deadlock. Aarch64 output takes no lock, so this
/// always prints and returns `Ok`.
pub fn try_print(args: fmt::Arguments) -> Result<(), ()> {
    use core::fmt::Write;
    crate::serial_line::TeeLine::new()
        .write_fmt(args)
        .map_err(|_| ())
}

/// Print from a panic or fatal path: `try_print`'s record, counted as unattributed
/// output (`output_accounting`).
pub fn emergency_print(args: fmt::Arguments) -> Result<(), ()> {
    use core::fmt::Write;
    crate::serial_line::TeeLine::unattributed()
        .write_fmt(args)
        .map_err(|_| ())
}

/// Write `s` as one record without waiting indefinitely on another CPU's output: up to 2^20
/// tries to own the UART without waiting, then the line goes out whole with the posted notices
/// (`log_notice::write_or_post`), half of whose nodes are kept for boot-path stage lines.
pub fn write_str_bounded(s: &str) {
    use crate::log_notice::Node;
    const ATTEMPTS: u32 = 1 << 20;
    static FALLBACK: [Node; 4] = [const { Node::new() }; 4];

    let stage = crate::stage_lines::contains(crate::STAGE_MARKERS, s.as_bytes());
    let line = s.strip_suffix('\n').unwrap_or(s);
    crate::log_notice::write_or_post(&FALLBACK, line, stage, ATTEMPTS, || {
        crate::serial_line::try_write(s.as_bytes())
    });
}

/// Wait until the terminal output written so far has left for the UART
/// (TCSETSW, TCSETSF). Kernel records are written before they return; queued
/// terminal output is written out here (`serial_line`).
pub fn drain_output() {
    crate::serial_line::drain_output();
}

/// Discard queued terminal output.
pub fn flush_output() {
    crate::serial_line::flush_output();
}

/// Stop or resume terminal output (IXON); queued output waits while stopped.
pub fn set_output_stopped(stopped: bool) {
    crate::serial_line::set_output_stopped(stopped);
}

/// Write out the lines posted on paths that may not log (`log_notice`).
pub fn write_notices() {
    crate::serial_line::write_notices();
}

/// Log print function for the log_serial_print macro
#[doc(hidden)]
pub fn _log_print(args: fmt::Arguments) {
    _print(args);
}

// =============================================================================
// Hardware output under UART ownership
// =============================================================================

/// The UART can accept a byte.
pub(crate) fn hardware_ready() -> bool {
    let addr = crate::platform_config::uart_virt() as usize;
    unsafe {
        if crate::platform_config::uart_type() == 1 {
            core::ptr::read_volatile((addr + reg16550::LSR) as *const u8) & reg16550::LSR_THRE != 0
        } else {
            core::ptr::read_volatile((addr + reg::FR) as *const u32) & flag::TXFF == 0
        }
    }
}

/// The UART byte sink. Callers must own the UART and have waited for
/// [`hardware_ready`] (see `serial_line`).
pub(crate) fn hardware_write(c: u8, _owner: &crate::serial_line::Ownership) {
    let addr = crate::platform_config::uart_virt() as usize;
    unsafe {
        if crate::platform_config::uart_type() == 1 {
            core::ptr::write_volatile(addr as *mut u8, c);
        } else {
            core::ptr::write_volatile(addr as *mut u32, c as u32);
        }
    }
}

#[macro_export]
macro_rules! serial_print {
    ($($arg:tt)*) => {
        $crate::serial_aarch64::_print(format_args!($($arg)*))
    };
}

#[macro_export]
macro_rules! serial_println {
    () => ($crate::serial_print!("\n"));
    ($($arg:tt)*) => ($crate::serial_print!("{}\n", format_args!($($arg)*)));
}

/// Print a kernel diagnostic line (a census or trace written for gates and debugging) to
/// serial and the kernel log, not to a screen console.
#[macro_export]
macro_rules! diag_println {
    ($($arg:tt)*) => ($crate::serial_aarch64::_diag_print(format_args!("{}\n", format_args!($($arg)*))));
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
