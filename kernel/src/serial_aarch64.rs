//! ARM64 serial output stub using PL011 UART.
//!
//! This is a stub implementation for ARM64. Full implementation will use
//! PL011 UART at MMIO address 0x0900_0000 (QEMU virt machine).

#![cfg(target_arch = "aarch64")]

use core::fmt;
use spin::Mutex;

/// PL011 UART base address for QEMU virt machine.
const PL011_BASE: usize = 0x0900_0000;

/// Stub serial port for ARM64.
pub struct SerialPort;

impl SerialPort {
    pub const fn new(_base: u16) -> Self {
        SerialPort
    }

    pub fn init(&mut self) {
        // TODO: Initialize PL011 UART
    }

    pub fn send(&mut self, byte: u8) {
        // Write directly to PL011 data register
        unsafe {
            let dr = PL011_BASE as *mut u32;
            core::ptr::write_volatile(dr, byte as u32);
        }
    }
}

pub static SERIAL1: Mutex<SerialPort> = Mutex::new(SerialPort::new(0));
pub static SERIAL2: Mutex<SerialPort> = Mutex::new(SerialPort::new(0));

pub fn init_serial() {
    SERIAL1.lock().init();
}

/// Write a single byte to serial output
pub fn write_byte(byte: u8) {
    // For ARM64, just write without interrupt disable for now
    // TODO: Add proper interrupt disable when GIC is implemented
    SERIAL1.lock().send(byte);
}

#[doc(hidden)]
pub fn _print(args: fmt::Arguments) {
    use core::fmt::Write;

    // For ARM64, just write without interrupt disable for now
    // TODO: Add proper interrupt disable when GIC is implemented
    let mut serial = SERIAL1.lock();
    let _ = write!(serial, "{}", args);
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

    // For ARM64, write directly to PL011 without locking
    struct EmergencySerial;

    impl fmt::Write for EmergencySerial {
        fn write_str(&mut self, s: &str) -> fmt::Result {
            for byte in s.bytes() {
                unsafe {
                    let dr = PL011_BASE as *mut u32;
                    core::ptr::write_volatile(dr, byte as u32);
                }
            }
            Ok(())
        }
    }

    let mut emergency = EmergencySerial;
    emergency.write_fmt(args).map_err(|_| ())?;
    Ok(())
}

impl fmt::Write for SerialPort {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for byte in s.bytes() {
            self.send(byte);
        }
        Ok(())
    }
}

/// Log print function for the log_serial_print macro
#[doc(hidden)]
pub fn _log_print(args: fmt::Arguments) {
    _print(args);
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
