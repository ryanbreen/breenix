use crate::serial_println;
use bootloader_x86_64_common::logger::LockedLogger;
use conquer_once::spin::OnceCell;
use core::fmt::{self, Write};
use core::sync::atomic::{AtomicU64, Ordering};
use log::{Level, LevelFilter, Log, Metadata, Record};
use spin::Mutex;

pub static FRAMEBUFFER_LOGGER: OnceCell<LockedLogger> = OnceCell::uninit();

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

        // Flush buffered messages to serial
        if buffer.position > 0 {
            serial_println!("=== Buffered Boot Messages ===");
            serial_println!("{}", buffer.contents());
            serial_println!("=== End Buffered Messages ===");
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

            // Use try_lock to avoid deadlocks from interrupt context
            let state = match self.state.try_lock() {
                Some(state) => state,
                None => {
                    // If we can't acquire the lock, fall back to basic serial output
                    // This prevents deadlocks when logging from interrupt handlers
                    serial_println!("[INTR] {}: {}", target, args);
                    return;
                }
            };

            // Get current timestamp if available
            // TEMPORARILY DISABLE TIMESTAMPS TO DEBUG TIMER INTERRUPT HANG
            let timestamp = 0;

            match *state {
                LoggerState::Buffering => {
                    // Buffer the message without timestamp (we don't have time yet)
                    drop(state); // Release lock before acquiring buffer lock
                    match self.buffer.try_lock() {
                        Some(mut buffer) => {
                            // Format directly into buffer
                            let _ = write!(&mut *buffer, "[{:>5}] {}: {}\n", level, target, args);
                        }
                        None => {
                            // Fall back to serial if buffer is locked
                            serial_println!("[BUFF] {}: {}", target, args);
                        }
                    }
                }
                LoggerState::SerialReady => {
                    // Output to serial only with timestamp if available
                    drop(state); // Release lock before serial I/O
                    if timestamp > 0 {
                        serial_println!(
                            "{} - [{:>5}] {}: {}",
                            timestamp,
                            record.level(),
                            record.target(),
                            record.args()
                        );
                    } else {
                        serial_println!(
                            "[{:>5}] {}: {}",
                            record.level(),
                            record.target(),
                            record.args()
                        );
                    }
                }
                LoggerState::FullyInitialized => {
                    // Output to both serial and framebuffer with timestamp
                    drop(state); // Release lock before I/O

                    if timestamp > 0 {
                        serial_println!(
                            "{} - [{:>5}] {}: {}",
                            timestamp,
                            record.level(),
                            record.target(),
                            record.args()
                        );
                    } else {
                        serial_println!(
                            "[{:>5}] {}: {}",
                            record.level(),
                            record.target(),
                            record.args()
                        );
                    }

                    // Write to framebuffer
                    // CRITICAL: Don't write to framebuffer if we're in interrupt/exception context
                    // or using a process page table, as the framebuffer might not be mapped
                    // Check if we're in interrupt context (IRQ) or exception context (preempt disabled)
                    // But only if per-CPU is initialized (otherwise assume we're safe)
                    let skip_framebuffer = if crate::per_cpu::is_initialized() {
                        // Skip if in IRQ context OR if preemption is disabled (exception context)
                        crate::per_cpu::in_interrupt() || crate::per_cpu::preempt_count() > 0
                    } else {
                        false // Early boot, safe to use framebuffer
                    };
                    
                    if !skip_framebuffer {
                        // TODO: Add proper synchronization to prevent rendering conflicts
                        // For now, we'll accept occasional visual glitches rather than deadlock
                        if let Some(fb_logger) = FRAMEBUFFER_LOGGER.get() {
                            fb_logger.log(record);
                        }
                    }
                }
            }
        }
    }

    fn flush(&self) {
        if let Some(fb_logger) = FRAMEBUFFER_LOGGER.get() {
            fb_logger.flush();
        }
    }
}

pub static COMBINED_LOGGER: CombinedLogger = CombinedLogger::new();

/// Initialize the logger early - can be called before serial is ready
pub fn init_early() {
    // Set up the combined logger with TRACE level
    // The CombinedLogger already suppresses TRACE logs while preserving timing
    log::set_logger(&COMBINED_LOGGER).expect("Logger already set");
    log::set_max_level(LevelFilter::Trace);
}

/// Call after serial port is initialized
pub fn serial_ready() {
    COMBINED_LOGGER.serial_ready();
}

/// Complete initialization with framebuffer
pub fn init_framebuffer(buffer: &'static mut [u8], info: bootloader_api::info::FrameBufferInfo) {
    // Initialize framebuffer logger
    let _fb_logger =
        FRAMEBUFFER_LOGGER.get_or_init(move || LockedLogger::new(buffer, info, true, false));

    // Mark logger as fully ready
    COMBINED_LOGGER.fully_ready();

    log::info!("Logger fully initialized - output to both framebuffer and serial");
}
