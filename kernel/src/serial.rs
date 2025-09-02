use conquer_once::spin::OnceCell;
use core::fmt;
use crossbeam_queue::ArrayQueue;
use futures_util::task::AtomicWaker;
use spin::Mutex;
use uart_16550::SerialPort;

pub mod command;

const COM1_PORT: u16 = 0x3F8;

pub static SERIAL1: Mutex<SerialPort> = Mutex::new(unsafe { SerialPort::new(COM1_PORT) });

// Serial input queue and waker (similar to keyboard)
static SERIAL_INPUT_QUEUE: OnceCell<ArrayQueue<u8>> = OnceCell::uninit();
static SERIAL_WAKER: AtomicWaker = AtomicWaker::new();

pub fn init() {
    // Initialize the serial port for output only (no interrupts yet)
    SERIAL1.lock().init();

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

    interrupts::without_interrupts(|| {
        SERIAL1.lock().send(byte);
    });
}

/// Flush the UART transmitter by waiting until both THR empty (THRE)
/// and transmitter empty (TEMT) bits are set in the Line Status Register.
/// This ensures all bytes have left the FIFO before returning.
pub fn flush() {
    use x86_64::instructions::interrupts;
    use x86_64::instructions::port::Port;
    const LSR_OFFSET: u16 = 5; // Line Status Register at base+5
    const LSR_THRE: u8 = 0x20; // Transmitter Holding Register Empty
    const LSR_TEMT: u8 = 0x40; // Transmitter Empty

    interrupts::without_interrupts(|| {
        let mut lsr: Port<u8> = Port::new(COM1_PORT + LSR_OFFSET);
        // Poll until both bits are set
        for _ in 0..1_000_000 {
            // Safety: reading I/O port
            let status = unsafe { lsr.read() };
            if (status & (LSR_THRE | LSR_TEMT)) == (LSR_THRE | LSR_TEMT) {
                break;
            }
        }
    });
}

#[doc(hidden)]
pub fn _print(args: fmt::Arguments) {
    use core::fmt::Write;
    use x86_64::instructions::interrupts;

    interrupts::without_interrupts(|| {
        SERIAL1
            .lock()
            .write_fmt(args)
            .expect("Printing to serial failed");
    });
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

pub struct SerialLogger;

impl SerialLogger {
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
            serial_println!(
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

pub struct SerialInputStream {
    _private: (),
}

impl SerialInputStream {
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
