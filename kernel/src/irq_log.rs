//! IRQ-safe logging infrastructure with per-CPU ring buffers
//!
//! This module provides a logging system that can be safely used from
//! interrupt context without deadlocks. It uses per-CPU ring buffers
//! to avoid locking in the interrupt path.

use core::fmt;
use core::sync::atomic::{AtomicUsize, AtomicBool, Ordering};

/// Size of each per-CPU log ring buffer (8 KiB)
const RING_BUFFER_SIZE: usize = 8192;

/// Log level for filtering
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum LogLevel {
    Error = 1,
    Warn = 2,
    Info = 3,
    Debug = 4,
    Trace = 5,
}

/// A single log entry in the ring buffer
#[repr(C)]
struct LogEntry {
    level: LogLevel,
    len: u16,
    // Message follows immediately after
}

/// Per-CPU log ring buffer
pub struct LogRing {
    /// The ring buffer itself
    buffer: [u8; RING_BUFFER_SIZE],
    /// Write position (only modified by local CPU with interrupts disabled)
    write_pos: AtomicUsize,
    /// Read position (only modified during flush)
    read_pos: AtomicUsize,
    /// Number of dropped messages due to overflow
    dropped: AtomicUsize,
    /// Recursion guard for flushing
    in_flush: AtomicBool,
}

impl LogRing {
    /// Create a new empty log ring
    const fn new() -> Self {
        Self {
            buffer: [0; RING_BUFFER_SIZE],
            write_pos: AtomicUsize::new(0),
            read_pos: AtomicUsize::new(0),
            dropped: AtomicUsize::new(0),
            in_flush: AtomicBool::new(false),
        }
    }

    /// Push a log message to the ring buffer
    /// MUST be called with local interrupts disabled on this CPU
    pub fn push(&mut self, level: LogLevel, args: fmt::Arguments) {
        // Format the message into a temporary buffer first
        let mut temp_buf = [0u8; 512]; // Max message size
        let mut writer = BufferWriter::new(&mut temp_buf);
        let _ = fmt::write(&mut writer, args);
        let msg_len = writer.pos;

        // Check if we have space (header + message)
        let entry_size = core::mem::size_of::<LogEntry>() + msg_len;
        let write_pos = self.write_pos.load(Ordering::Relaxed);
        let read_pos = self.read_pos.load(Ordering::Relaxed);
        
        let space_used = if write_pos >= read_pos {
            write_pos - read_pos
        } else {
            RING_BUFFER_SIZE - read_pos + write_pos
        };
        
        let space_available = RING_BUFFER_SIZE - space_used - 1; // -1 to distinguish full from empty
        
        if entry_size > space_available {
            // Buffer overflow - drop the message
            self.dropped.fetch_add(1, Ordering::Relaxed);
            return;
        }

        // Write the entry header
        let entry = LogEntry {
            level,
            len: msg_len as u16,
        };
        
        // Copy header to buffer
        let header_bytes = unsafe {
            core::slice::from_raw_parts(
                &entry as *const _ as *const u8,
                core::mem::size_of::<LogEntry>()
            )
        };
        
        let mut pos = write_pos;
        for &byte in header_bytes {
            self.buffer[pos] = byte;
            pos = (pos + 1) % RING_BUFFER_SIZE;
        }
        
        // Copy message to buffer
        for i in 0..msg_len {
            self.buffer[pos] = temp_buf[i];
            pos = (pos + 1) % RING_BUFFER_SIZE;
        }
        
        // Update write position
        self.write_pos.store(pos, Ordering::Release);
    }

    /// Try to flush the ring buffer to serial
    /// Returns true if flush was performed, false if already flushing
    pub fn try_flush(&mut self) -> bool {
        // Check recursion guard
        if self.in_flush.swap(true, Ordering::Acquire) {
            return false; // Already flushing
        }

        // Check if there are dropped messages to report
        let dropped = self.dropped.swap(0, Ordering::Relaxed);
        if dropped > 0 {
            // Try to log dropped message count
            // This should go directly to serial if possible
            let _ = crate::serial::try_print(format_args!(
                "[IRQ_LOG] Dropped {} messages\n", dropped
            ));
        }

        // Flush all pending messages
        let mut flushed = false;
        loop {
            let read_pos = self.read_pos.load(Ordering::Acquire);
            let write_pos = self.write_pos.load(Ordering::Acquire);
            
            if read_pos == write_pos {
                break; // Buffer is empty
            }
            
            // Read the entry header
            let mut header_bytes = [0u8; core::mem::size_of::<LogEntry>()];
            let mut pos = read_pos;
            for byte in &mut header_bytes {
                *byte = self.buffer[pos];
                pos = (pos + 1) % RING_BUFFER_SIZE;
            }
            
            let entry = unsafe {
                core::ptr::read(header_bytes.as_ptr() as *const LogEntry)
            };
            
            // Read the message
            let mut msg_buf = [0u8; 512];
            let msg_len = entry.len as usize;
            for i in 0..msg_len {
                msg_buf[i] = self.buffer[pos];
                pos = (pos + 1) % RING_BUFFER_SIZE;
            }
            
            // Try to output the message
            if let Ok(msg) = core::str::from_utf8(&msg_buf[..msg_len]) {
                let level_str = match entry.level {
                    LogLevel::Error => "ERROR",
                    LogLevel::Warn => "WARN",
                    LogLevel::Info => "INFO",
                    LogLevel::Debug => "DEBUG",
                    LogLevel::Trace => "TRACE",
                };
                
                // Try to send to serial
                let _ = crate::serial::try_print(format_args!(
                    "[{}] {}\n", level_str, msg
                ));
            }
            
            // Update read position
            self.read_pos.store(pos, Ordering::Release);
            flushed = true;
        }

        // Clear recursion guard
        self.in_flush.store(false, Ordering::Release);
        flushed
    }
}

/// Simple buffer writer for formatting
struct BufferWriter<'a> {
    buffer: &'a mut [u8],
    pos: usize,
}

impl<'a> BufferWriter<'a> {
    fn new(buffer: &'a mut [u8]) -> Self {
        Self { buffer, pos: 0 }
    }
}

impl<'a> fmt::Write for BufferWriter<'a> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let bytes = s.as_bytes();
        let remaining = self.buffer.len() - self.pos;
        let to_write = bytes.len().min(remaining);
        
        self.buffer[self.pos..self.pos + to_write]
            .copy_from_slice(&bytes[..to_write]);
        self.pos += to_write;
        
        if to_write < bytes.len() {
            Err(fmt::Error) // Buffer full
        } else {
            Ok(())
        }
    }
}

/// Per-CPU log ring storage
static mut CPU0_LOG_RING: LogRing = LogRing::new();

/// Get the current CPU's log ring
/// SAFETY: Must be called with interrupts disabled or from interrupt context
pub unsafe fn get_log_ring() -> &'static mut LogRing {
    // For now, we only support CPU 0
    // TODO: Use proper per-CPU infrastructure
    &mut CPU0_LOG_RING
}

/// Main IRQ-safe logging function
pub fn irq_safe_log(level: LogLevel, args: fmt::Arguments) {
    // TEMPORARY: Bypass IRQ-safe logging to debug hang
    // Just try to print directly, ignore failures
    let _ = crate::serial::try_print(args);
    return;
    
    // Original implementation disabled to debug hang:
    /*
    // Check if we're in interrupt context
    let in_interrupt = crate::per_cpu::in_interrupt();
    
    if in_interrupt {
        // In interrupt context - just push to ring buffer
        // Disable interrupts to prevent nested interrupts while modifying ring
        x86_64::instructions::interrupts::without_interrupts(|| {
            unsafe {
                get_log_ring().push(level, args);
            }
        });
        
        // Try opportunistic flush with try-lock
        unsafe {
            let _ = get_log_ring().try_flush();
        }
    } else {
        // Normal context - try to log directly
        if crate::serial::try_print(args).is_err() {
            // Serial is locked - buffer the message
            x86_64::instructions::interrupts::without_interrupts(|| {
                unsafe {
                    get_log_ring().push(level, args);
                }
            });
        } else {
            // After successful direct log, try to flush any buffered messages
            x86_64::instructions::interrupts::without_interrupts(|| {
                unsafe {
                    let _ = get_log_ring().try_flush();
                }
            });
        }
    }
    */
}

/// Try to flush the local CPU's log ring (non-blocking)
pub fn flush_local_try() {
    x86_64::instructions::interrupts::without_interrupts(|| {
        unsafe {
            let _ = get_log_ring().try_flush();
        }
    });
}

/// Emergency logging function for panics
pub fn emergency_log(args: fmt::Arguments) {
    // Try direct serial output first (polling mode)
    if crate::serial::emergency_print(args).is_ok() {
        return;
    }
    
    // Fall back to ring buffer if serial is completely broken
    x86_64::instructions::interrupts::without_interrupts(|| {
        unsafe {
            get_log_ring().push(LogLevel::Error, args);
        }
    });
}

/// Macro for IRQ-safe logging
#[macro_export]
macro_rules! irq_log {
    ($level:expr, $($arg:tt)*) => {
        $crate::irq_log::irq_safe_log($level, format_args!($($arg)*))
    };
}

/// Convenience macros for different log levels
#[macro_export]
macro_rules! irq_error {
    ($($arg:tt)*) => {
        $crate::irq_log!($crate::irq_log::LogLevel::Error, $($arg)*)
    };
}

#[macro_export]
macro_rules! irq_info {
    ($($arg:tt)*) => {
        $crate::irq_log!($crate::irq_log::LogLevel::Info, $($arg)*)
    };
}

#[macro_export]
macro_rules! irq_debug {
    ($($arg:tt)*) => {
        $crate::irq_log!($crate::irq_log::LogLevel::Debug, $($arg)*)
    };
}