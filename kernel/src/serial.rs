use conquer_once::spin::OnceCell;
use core::fmt;
use crossbeam_queue::ArrayQueue;
use futures_util::task::AtomicWaker;
use spin::Mutex;
use uart_16550::SerialPort;

pub mod command;

const COM1_PORT: u16 = 0x3F8;
const COM2_PORT: u16 = 0x2F8;

pub static SERIAL1: Mutex<SerialPort> = Mutex::new(unsafe { SerialPort::new(COM1_PORT) });

/// COM2 (0x2F8) - Used exclusively for kernel log output.
/// This separates kernel logs from user I/O (COM1), enabling interactive shell use.
pub static SERIAL2: Mutex<SerialPort> = Mutex::new(unsafe { SerialPort::new(COM2_PORT) });

// Serial input queue and waker (similar to keyboard)
static SERIAL_INPUT_QUEUE: OnceCell<ArrayQueue<u8>> = OnceCell::uninit();
static SERIAL_WAKER: AtomicWaker = AtomicWaker::new();

pub fn init() {
    // Initialize the serial port for output only (no interrupts yet)
    SERIAL1.lock().init();

    // Initialize COM2 for kernel log output
    SERIAL2.lock().init();

    // Don't enable interrupts here - wait until after IDT is set up
}

/// Enable serial input interrupts - call this after IDT and PIC are initialized
pub fn enable_serial_input() {
    // Initialize the input queue
    SERIAL_INPUT_QUEUE
        .try_init_once(|| ArrayQueue::new(256))
        .expect("Serial input queue already initialized");

    // Enable receive interrupts (when data is available)
    unsafe {
        use x86_64::instructions::port::Port;

        // Read current interrupt enable register
        let mut ier_port: Port<u8> = Port::new(COM1_PORT + 1);
        let mut ier = ier_port.read();

        // Set bit 0 to enable "data available" interrupts
        ier |= 0x01;
        ier_port.write(ier);
    }

    log::info!("Serial input interrupts enabled");
}

/// Called by the serial interrupt handler
///
/// Must not block or allocate.
pub fn add_serial_byte(byte: u8) {
    if let Ok(queue) = SERIAL_INPUT_QUEUE.try_get() {
        if let Err(_) = queue.push(byte) {
            log::warn!("Serial input queue full; dropping input");
        } else {
            SERIAL_WAKER.wake();
        }
    } else {
        log::warn!("Serial input queue uninitialized");
    }
}

pub fn write_byte(byte: u8) {
    use x86_64::instructions::interrupts;

    // CRITICAL: Check if interrupts are currently enabled
    // We must NOT re-enable interrupts if they were disabled by syscall entry
    let irq_enabled = interrupts::are_enabled();

    // Disable interrupts while holding the lock
    interrupts::disable();

    SERIAL1.lock().send(byte);

    // Only re-enable if they were enabled before
    // This prevents race condition in syscall handler where interrupts must stay disabled
    if irq_enabled {
        interrupts::enable();
    }
}

#[doc(hidden)]
pub fn _print(args: fmt::Arguments) {
    use core::fmt::Write;
    use x86_64::instructions::interrupts;

    // CRITICAL FIX: Check if interrupts are currently enabled BEFORE disabling
    // We must NOT re-enable interrupts if they were disabled by syscall entry
    // This fixes the RDI corruption bug where timer interrupts fire during syscalls
    let irq_enabled = interrupts::are_enabled();

    // Disable interrupts while holding the lock
    interrupts::disable();

    SERIAL1
        .lock()
        .write_fmt(args)
        .expect("Printing to serial failed");

    // Only re-enable if they were enabled before
    // This prevents race condition in syscall handler where interrupts must stay disabled
    if irq_enabled {
        interrupts::enable();
    }
}

/// Try to print without blocking - returns Err if lock is held
pub fn try_print(args: fmt::Arguments) -> Result<(), ()> {
    use core::fmt::Write;
    use x86_64::instructions::interrupts;

    // CRITICAL: Check if interrupts are currently enabled
    // We must NOT re-enable interrupts if they were disabled by syscall entry
    let irq_enabled = interrupts::are_enabled();

    // Disable interrupts while holding the lock
    interrupts::disable();

    let result = match SERIAL1.try_lock() {
        Some(mut serial) => {
            serial.write_fmt(args).map_err(|_| ())?;
            Ok(())
        }
        None => Err(()), // Lock is held
    };

    // Only re-enable if they were enabled before
    // This prevents race condition in syscall handler where interrupts must stay disabled
    if irq_enabled {
        interrupts::enable();
    }

    result
}

/// Emergency print for panics - uses direct port I/O without locking
/// WARNING: May corrupt output if racing with normal serial output
#[allow(dead_code)]
pub fn emergency_print(args: fmt::Arguments) -> Result<(), ()> {
    use core::fmt::Write;
    use x86_64::instructions::interrupts;
    
    // Use a simple global flag to reduce corruption
    static EMERGENCY_IN_USE: core::sync::atomic::AtomicBool = 
        core::sync::atomic::AtomicBool::new(false);
    
    // Try to claim exclusive emergency access
    if EMERGENCY_IN_USE.swap(true, core::sync::atomic::Ordering::Acquire) {
        return Err(()); // Someone else is using emergency path
    }
    
    // Write directly to serial port without locking
    // This is unsafe but necessary for panic handling
    struct EmergencySerial;
    
    impl fmt::Write for EmergencySerial {
        fn write_str(&mut self, s: &str) -> fmt::Result {
            for byte in s.bytes() {
                unsafe {
                    // Direct port I/O to COM1
                    x86_64::instructions::port::Port::<u8>::new(0x3F8).write(byte);
                }
            }
            Ok(())
        }
    }
    
    interrupts::without_interrupts(|| {
        let mut emergency = EmergencySerial;
        let result = emergency.write_fmt(args).map_err(|_| ());
        
        // Release emergency access
        EMERGENCY_IN_USE.store(false, core::sync::atomic::Ordering::Release);
        
        result
    })
}

/// Flush serial output
#[allow(dead_code)]
pub fn flush_serial() {
    // For UART, there's not much to flush - it's synchronous
    // But we can ensure the transmit holding register is empty
    unsafe {
        use x86_64::instructions::port::Port;
        let mut status_port = Port::<u8>::new(0x3F8 + 5);
        while (status_port.read() & 0x20) == 0 {
            core::hint::spin_loop();
        }
    }
}

#[macro_export]
macro_rules! serial_print {
    ($($arg:tt)*) => ($crate::serial::_print(format_args!($($arg)*)));
}

#[macro_export]
macro_rules! serial_println {
    () => ($crate::serial_print!("\n"));
    ($($arg:tt)*) => ($crate::serial_print!("{}\n", format_args!($($arg)*)));
}

/// Print to the log serial port (COM2).
/// This is used for kernel log output to separate it from user I/O on COM1.
#[doc(hidden)]
pub fn _log_print(args: fmt::Arguments) {
    use core::fmt::Write;
    use x86_64::instructions::interrupts;

    // CRITICAL: Check if interrupts are currently enabled BEFORE disabling
    // We must NOT re-enable interrupts if they were disabled by syscall entry
    let irq_enabled = interrupts::are_enabled();

    // Disable interrupts while holding the lock
    interrupts::disable();

    SERIAL2
        .lock()
        .write_fmt(args)
        .expect("Printing to log serial failed");

    // Only re-enable if they were enabled before
    if irq_enabled {
        interrupts::enable();
    }
}

/// Macros for writing to the log serial port (COM2).
/// Use these for kernel debug/info output that should not appear on user console.
#[macro_export]
macro_rules! log_serial_print {
    ($($arg:tt)*) => ($crate::serial::_log_print(format_args!($($arg)*)));
}

#[macro_export]
macro_rules! log_serial_println {
    () => ($crate::log_serial_print!("\n"));
    ($($arg:tt)*) => ($crate::log_serial_print!("{}\n", format_args!($($arg)*)));
}

#[allow(dead_code)]
pub struct SerialLogger;

impl SerialLogger {
    #[allow(dead_code)]
    pub const fn new() -> Self {
        SerialLogger
    }
}

impl Default for SerialLogger {
    fn default() -> Self {
        Self::new()
    }
}

impl log::Log for SerialLogger {
    fn enabled(&self, _metadata: &log::Metadata) -> bool {
        true
    }

    fn log(&self, record: &log::Record) {
        if self.enabled(record.metadata()) {
            // Use COM2 for log output to separate from user I/O on COM1
            log_serial_println!(
                "[{}] {}: {}",
                record.level(),
                record.target(),
                record.args()
            );
        }
    }

    fn flush(&self) {}
}

// Serial input stream implementation
use core::{
    pin::Pin,
    task::{Context, Poll},
};
use futures_util::stream::Stream;

#[allow(dead_code)] // Used in serial_command_task (conditionally compiled)
pub struct SerialInputStream {
    _private: (),
}

impl SerialInputStream {
    #[allow(dead_code)] // Used in serial_command_task (conditionally compiled)
    pub fn new() -> Self {
        // Ensure queue is initialized
        let _ = SERIAL_INPUT_QUEUE.try_init_once(|| ArrayQueue::new(256));

        SerialInputStream { _private: () }
    }
}

impl Stream for SerialInputStream {
    type Item = u8;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Option<u8>> {
        let queue = SERIAL_INPUT_QUEUE
            .try_get()
            .expect("serial input queue not initialized");

        // Fast path
        if let Some(byte) = queue.pop() {
            return Poll::Ready(Some(byte));
        }

        SERIAL_WAKER.register(&cx.waker());
        match queue.pop() {
            Some(byte) => {
                SERIAL_WAKER.take();
                Poll::Ready(Some(byte))
            }
            None => Poll::Pending,
        }
    }
}
