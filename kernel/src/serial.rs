use uart_16550::SerialPort;
use spin::Mutex;
use core::fmt;

const COM1_PORT: u16 = 0x3F8;

pub static SERIAL1: Mutex<SerialPort> = Mutex::new(unsafe { SerialPort::new(COM1_PORT) });

pub fn init() {
    SERIAL1.lock().init();
}

#[doc(hidden)]
pub fn _print(args: fmt::Arguments) {
    use core::fmt::Write;
    use x86_64::instructions::interrupts;
    
    interrupts::without_interrupts(|| {
        SERIAL1.lock()
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

impl log::Log for SerialLogger {
    fn enabled(&self, _metadata: &log::Metadata) -> bool {
        true
    }

    fn log(&self, record: &log::Record) {
        if self.enabled(record.metadata()) {
            serial_println!("[{}] {}: {}", 
                record.level(), 
                record.target(), 
                record.args()
            );
        }
    }

    fn flush(&self) {}
}

pub static LOGGER: SerialLogger = SerialLogger::new();

pub fn test_serial() {
    serial_println!("Serial port initialized successfully!");
    serial_println!("This is a test message from the serial port.");
}