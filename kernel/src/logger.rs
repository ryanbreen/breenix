//! Logging infrastructure for Breenix kernel.
//!
//! This module is currently x86_64-only due to bootloader dependencies.

#![cfg(target_arch = "x86_64")]

use crate::log_serial_println;
use core::fmt::{self, Write};
use core::sync::atomic::{AtomicU64, Ordering};
use log::{Level, LevelFilter, Log, Metadata, Record};
use spin::Mutex;

const BUFFER_SIZE: usize = 8192;

/// Counting sink to suppress TRACE logs while preserving timing
/// This prevents log spam while maintaining the synchronization side effects
#[allow(dead_code)]
struct CountingSink(AtomicU64);

impl CountingSink {
    #[allow(dead_code)]
    const fn new() -> Self {
        CountingSink(AtomicU64::new(0))
    }
}

impl Log for CountingSink {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() == Level::Trace
    }

    fn log(&self, record: &Record) {
        if self.enabled(record.metadata()) {
            // Increment counter but don't output
            self.0.fetch_add(1, Ordering::Relaxed);

            // Preserve all the timing side effects of log processing
            let _level = record.level();
            let _target = record.target();
            let _args = record.args();

            // This format_args call is crucial - it forces evaluation of the arguments
            // which preserves the timing behavior that prevents the race condition
            let _ = format_args!("{}", _args);
        }
    }

    fn flush(&self) {
        // No-op for counting sink
    }
}

#[allow(dead_code)]
static COUNTING_SINK: CountingSink = CountingSink::new();

/// Buffer for storing log messages before serial is initialized
struct LogBuffer {
    buffer: [u8; BUFFER_SIZE],
    position: usize,
}

impl LogBuffer {
    const fn new() -> Self {
        Self {
            buffer: [0; BUFFER_SIZE],
            position: 0,
        }
    }

    fn write_str(&mut self, s: &str) -> fmt::Result {
        let bytes = s.as_bytes();
        let remaining = BUFFER_SIZE - self.position;

        if bytes.len() > remaining {
            // Buffer is full, drop oldest messages to make room
            // Simple strategy: just keep the newer messages
            return Ok(());
        }

        self.buffer[self.position..self.position + bytes.len()].copy_from_slice(bytes);
        self.position += bytes.len();

        Ok(())
    }

    fn contents(&self) -> &str {
        core::str::from_utf8(&self.buffer[..self.position]).unwrap_or("<invalid UTF-8>")
    }
}

impl Write for LogBuffer {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.write_str(s)
    }
}

/// State of the logger
#[derive(Clone, Copy)]
enum LoggerState {
    /// Buffering messages until serial is ready
    Buffering,
    /// Serial is initialized, flush buffer and start outputting
    SerialReady,
    /// Fully initialized with framebuffer
    FullyInitialized,
}

pub struct CombinedLogger {
    buffer: Mutex<LogBuffer>,
    state: Mutex<LoggerState>,
}

impl CombinedLogger {
    const fn new() -> Self {
        CombinedLogger {
            buffer: Mutex::new(LogBuffer::new()),
            state: Mutex::new(LoggerState::Buffering),
        }
    }

    /// Call this after serial is initialized
    pub fn serial_ready(&self) {
        let mut state = self.state.lock();
        let buffer = self.buffer.lock();

        // Flush buffered messages to log serial (COM2)
        if buffer.position > 0 {
            log_serial_println!("=== Buffered Boot Messages ===");
            log_serial_println!("{}", buffer.contents());
            log_serial_println!("=== End Buffered Messages ===");
        }

        *state = LoggerState::SerialReady;
    }

    /// Call this after framebuffer logger is initialized
    pub fn fully_ready(&self) {
        let mut state = self.state.lock();
        *state = LoggerState::FullyInitialized;
    }
}

impl Log for CombinedLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= Level::Trace
    }

    fn log(&self, record: &Record) {
        if self.enabled(record.metadata()) {
            // SUPPRESS TRACE LOGS TO REDUCE OUTPUT WHILE PRESERVING TIMING
            if record.level() == Level::Trace {
                // Process the log record to preserve timing but don't output
                let _level = record.level();
                let _target = record.target();
                let _args = record.args();
                let _ = format_args!("{}", _args);
                return;
            }
            // Format the message manually to avoid allocation
            let level = record.level();
            let target = record.target();
            let args = record.args();

            // Use try_lock to avoid deadlocks from interrupt context, and copy
            // the state out with interrupts masked, as the framebuffer sink
            // does, so no holder is ever preempted with the lock held. The x86
            // boot thread is never scheduled again once it drops its preempt
            // pin; a tick inside this window used to leave the lock held for
            // the rest of the boot, sending every later record down this
            // fallback.
            let state = match crate::arch_without_interrupts(|| {
                self.state.try_lock().map(|state| *state)
            }) {
                Some(state) => state,
                None => {
                    // If we can't acquire the lock, fall back to basic log serial output (COM2)
                    // This prevents deadlocks when logging from interrupt handlers
                    log_serial_println!("[INTR] {}: {}", target, args);
                    return;
                }
            };

            // Get current timestamp if available
            // TEMPORARILY DISABLE TIMESTAMPS TO DEBUG TIMER INTERRUPT HANG
            let timestamp = 0;

            match state {
                LoggerState::Buffering => {
                    // Buffer the message without timestamp (we don't have time yet)
                    match self.buffer.try_lock() {
                        Some(mut buffer) => {
                            // Format directly into buffer
                            let _ = write!(&mut *buffer, "[{:>5}] {}: {}\n", level, target, args);
                        }
                        None => {
                            // Fall back to log serial (COM2) if buffer is locked
                            log_serial_println!("[BUFF] {}: {}", target, args);
                        }
                    }
                }
                LoggerState::SerialReady => {
                    // Output to log serial (COM2) only with timestamp if available
                    if timestamp > 0 {
                        log_serial_println!(
                            "{} - [{:>5}] {}: {}",
                            timestamp,
                            record.level(),
                            record.target(),
                            record.args()
                        );
                    } else {
                        log_serial_println!(
                            "[{:>5}] {}: {}",
                            record.level(),
                            record.target(),
                            record.args()
                        );
                    }
                }
                LoggerState::FullyInitialized => {
                    // Output to log serial (COM2) - always

                    if timestamp > 0 {
                        log_serial_println!(
                            "{} - [{:>5}] {}: {}",
                            timestamp,
                            record.level(),
                            record.target(),
                            record.args()
                        );
                    } else {
                        log_serial_println!(
                            "[{:>5}] {}: {}",
                            record.level(),
                            record.target(),
                            record.args()
                        );
                    }

                    // Also put the record on the screen console, which takes
                    // records from any context.
                    crate::console::log_record(record.level(), record.args());
                }
            }
        }
    }

    fn flush(&self) {}
}

pub static COMBINED_LOGGER: CombinedLogger = CombinedLogger::new();

/// Initialize the logger early - can be called before serial is ready
pub fn init_early() {
    // Records above INFO are compiled in but not emitted. A record is a
    // polled write to COM2, and on a virtualized x86 host each byte costs a
    // port-I/O exit, so DEBUG output (482 of 926 KB in a testing gate boot
    // of 370a233b) set the pace of the boot. Raise this to LevelFilter::Debug
    // locally when a debug trace is wanted.
    log::set_logger(&COMBINED_LOGGER).expect("Logger already set");
    log::set_max_level(LevelFilter::Info);
}

/// Call after serial port is initialized
pub fn serial_ready() {
    COMBINED_LOGGER.serial_ready();
}

/// Complete initialization with framebuffer: the screen console keeps log
/// records from here.
pub fn init_framebuffer(buffer: &'static mut [u8], info: bootloader_api::info::FrameBufferInfo) {
    crate::console::init_early(buffer, info);

    // Mark logger as fully ready
    COMBINED_LOGGER.fully_ready();

    log::info!("Logger fully initialized - output to both framebuffer and serial");
}
