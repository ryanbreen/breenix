#![cfg(target_arch = "x86_64")]

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

static SERIAL1_FIFO: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
static SERIAL2_FIFO: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Kernel log bytes not yet written to COM2.
///
/// At 115200 baud the UART takes 87 us a byte, so the strand census's
/// 330-byte line keeps COM2 busy for 29 ms, and the x86 gate's emulation
/// stretches that to about 60 ms. Written under SERIAL2 with interrupts
/// masked, that line held its CPU for the whole time: a wake sent there,
/// the thread it had just switched away from, and any CPU logging behind it
/// (some while holding the scheduler or process-manager lock) all waited.
/// Lines are appended here instead, whole and under a lock held only for
/// the copy, and one printer at a time writes them out (`drain_log`),
/// waiting on the UART with interrupts as its caller had them.
const LOG_RING_BYTES: usize = 64 * 1024;

struct LogRing {
    bytes: [u8; LOG_RING_BYTES],
    head: usize,
    len: usize,
    dropped: u64,
}

impl LogRing {
    fn push(&mut self, byte: u8) -> bool {
        if self.len == LOG_RING_BYTES {
            return false;
        }
        self.bytes[(self.head + self.len) % LOG_RING_BYTES] = byte;
        self.len += 1;
        true
    }

    fn pop(&mut self, out: &mut [u8]) -> usize {
        let n = out.len().min(self.len);
        for slot in &mut out[..n] {
            *slot = self.bytes[self.head];
            self.head = (self.head + 1) % LOG_RING_BYTES;
        }
        self.len -= n;
        n
    }
}

static LOG_RING: Mutex<LogRing> = Mutex::new(LogRing {
    bytes: [0; LOG_RING_BYTES],
    head: 0,
    len: 0,
    dropped: 0,
});

/// Held by the one context writing the kernel log to COM2.
static LOG_PRINTER: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Appends one formatted line to the log ring, which its caller holds.
struct RingWriter<'a>(&'a mut LogRing);

impl fmt::Write for RingWriter<'_> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for byte in s.bytes() {
            // uart_16550::send translates backspace and DEL to BS SPACE BS.
            let wire: &[u8] = if byte == 0x08 || byte == 0x7f { &[0x08, b' ', 0x08] } else { core::slice::from_ref(&byte) };
            for &b in wire {
                if !self.0.push(b) && !(make_room(self.0) && self.0.push(b)) {
                    self.0.dropped += 1;
                }
            }
        }
        Ok(())
    }
}

/// A full ring, with no printer: write one burst out from under the ring lock.
/// With a printer active the byte is dropped instead, and counted.
fn make_room(ring: &mut LogRing) -> bool {
    if LOG_PRINTER
        .compare_exchange(false, true, core::sync::atomic::Ordering::Acquire, core::sync::atomic::Ordering::Relaxed)
        .is_err()
    {
        return false;
    }
    let mut chunk = [0u8; 16];
    let n = ring.pop(&mut chunk[..log_burst()]);
    write_log_burst(&chunk[..n]);
    LOG_PRINTER.store(false, core::sync::atomic::Ordering::Release);
    n != 0
}

fn log_burst() -> usize {
    if SERIAL2_FIFO.load(core::sync::atomic::Ordering::Relaxed) { 16 } else { 1 }
}

/// Wait for COM2's transmitter to empty, then write `bytes`, at most one FIFO.
fn write_log_burst(bytes: &[u8]) {
    use x86_64::instructions::port::Port;
    // SAFETY: COM2's status and data ports; only the printer writes COM2.
    unsafe {
        let mut status = Port::<u8>::new(COM2_PORT + 5);
        let mut data = Port::<u8>::new(COM2_PORT);
        while status.read() & 0x20 == 0 {
            core::hint::spin_loop();
        }
        for &byte in bytes {
            data.write(byte);
        }
    }
}

/// Write the log ring to COM2 until it is empty, unless another context is
/// already doing so: that printer takes this caller's bytes too.
///
/// The UART wait runs with interrupts as the caller had them, and no lock is
/// held across it; the ring lock is taken, with interrupts masked, only to take
/// each FIFO's worth. An interrupt on this CPU that logs meanwhile finds the
/// printer busy and only appends.
fn drain_log() {
    use core::sync::atomic::Ordering;
    loop {
        if LOG_PRINTER
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            return;
        }
        crate::task::wakelat::ev(crate::task::wakelat::E_LOG_IN, 2);
        loop {
            let mut chunk = [0u8; 16];
            let n = crate::arch_without_interrupts(|| LOG_RING.lock().pop(&mut chunk[..log_burst()]));
            if n == 0 {
                break;
            }
            write_log_burst(&chunk[..n]);
        }
        crate::task::wakelat::ev(crate::task::wakelat::E_LOG_OUT, 2);
        LOG_PRINTER.store(false, Ordering::Release);
        // A line appended after the last take saw the printer busy and left.
        if crate::arch_without_interrupts(|| LOG_RING.lock().len) == 0 {
            return;
        }
    }
}

/// Write out whatever the kernel log still holds, from a panic, whoever holds
/// the printer role. Bounded: a ring lock this CPU holds is left alone.
pub fn flush_log_for_panic() {
    const ATTEMPTS: u32 = 1 << 20;
    crate::arch_without_interrupts(|| {
        for _ in 0..ATTEMPTS {
            if let Some(mut ring) = LOG_RING.try_lock() {
                let mut chunk = [0u8; 16];
                loop {
                    let n = ring.pop(&mut chunk[..log_burst()]);
                    if n == 0 {
                        return;
                    }
                    write_log_burst(&chunk[..n]);
                }
            }
            core::hint::spin_loop();
        }
    });
}

pub fn init() {
    // Initialize the serial port for output only (no interrupts yet)
    init_port(&mut SERIAL1.lock(), COM1_PORT);
    let fifo = unsafe {
        use x86_64::instructions::port::Port;
        Port::<u8>::new(COM1_PORT + 2).read() & 0xc0 == 0xc0
    };
    SERIAL1_FIFO.store(fifo, core::sync::atomic::Ordering::Relaxed);

    // Initialize COM2 for kernel log output
    init_port(&mut SERIAL2.lock(), COM2_PORT);
    let fifo = unsafe {
        use x86_64::instructions::port::Port;
        Port::<u8>::new(COM2_PORT + 2).read() & 0xc0 == 0xc0
    };
    SERIAL2_FIFO.store(fifo, core::sync::atomic::Ordering::Relaxed);

    // Don't enable interrupts here - wait until after IDT is set up
}

/// Use the PC console's 115200 baud rather than the library's 38400 default.
/// Synchronous console output otherwise masks interrupts for over 20 ms for
/// an ordinary 80-byte line, delaying timer delivery and charging that busy
/// UART wait to the writing process's CPU clock.
fn init_port(serial: &mut SerialPort, base: u16) {
    serial.init();
    unsafe {
        use x86_64::instructions::port::Port;
        Port::<u8>::new(base + 3).write(0x80); // DLAB
        Port::<u8>::new(base).write(1);       // 115200 / 1
        Port::<u8>::new(base + 1).write(0);
        Port::<u8>::new(base + 3).write(0x03); // 8N1, DLAB clear
    }
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
    write_bytes_atomic(core::slice::from_ref(&byte));
}

/// Write one caller-defined output unit while holding the serial lock once.
pub fn write_bytes_atomic(bytes: &[u8]) {
    // CRITICAL: Check if interrupts are currently enabled
    // We must NOT re-enable interrupts if they were disabled by syscall entry
    let irq_enabled = crate::arch_interrupts_enabled();

    // Disable interrupts while holding the lock
    unsafe {
        crate::arch_disable_interrupts();
    }

    crate::task::wakelat::ev(crate::task::wakelat::E_LOG_IN, 1);
    let mut serial = SERIAL1.lock();
    if SERIAL1_FIFO.load(core::sync::atomic::Ordering::Relaxed) {
        // THRE means the 16550 transmit FIFO is empty, so all sixteen slots
        // are available. Poll once per burst, avoiding a VM exit and a UART
        // character-time spin for every individual byte.
        unsafe {
            use x86_64::instructions::port::Port;
            let mut status = Port::<u8>::new(COM1_PORT + 5);
            let mut data = Port::<u8>::new(COM1_PORT);
            let mut chunk = [0u8; 16];
            let mut used = 0;
            for &byte in bytes {
                // uart_16550::send translates backspace and DEL to BS SPACE BS.
                // Apply that translation before filling the hardware FIFO too.
                let wire = if byte == 0x08 || byte == 0x7f {
                    [0x08, b' ', 0x08]
                } else {
                    [byte, 0, 0]
                };
                let count = if byte == 0x08 || byte == 0x7f { 3 } else { 1 };
                for &transmitted in &wire[..count] {
                    chunk[used] = transmitted;
                    used += 1;
                    if used == chunk.len() {
                        while status.read() & 0x20 == 0 { core::hint::spin_loop(); }
                        for &pending in &chunk { data.write(pending); }
                        used = 0;
                    }
                }
                crate::log_buffer::capture_byte(byte);
            }
            if used != 0 {
                while status.read() & 0x20 == 0 { core::hint::spin_loop(); }
                for &pending in &chunk[..used] { data.write(pending); }
            }
        }
    } else {
        for &byte in bytes {
            serial.send(byte);
            crate::log_buffer::capture_byte(byte);
        }
    }
    drop(serial);
    crate::task::wakelat::ev(crate::task::wakelat::E_LOG_OUT, 1);

    // Only re-enable if they were enabled before
    // This prevents race condition in syscall handler where interrupts must stay disabled
    if irq_enabled {
        unsafe {
            crate::arch_enable_interrupts();
        }
    }
}

#[doc(hidden)]
pub fn _print(args: fmt::Arguments) {
    use core::fmt::Write;

    // CRITICAL FIX: Check if interrupts are currently enabled BEFORE disabling
    // We must NOT re-enable interrupts if they were disabled by syscall entry
    // This fixes the RDI corruption bug where timer interrupts fire during syscalls
    let irq_enabled = crate::arch_interrupts_enabled();

    // Disable interrupts while holding the lock
    unsafe {
        crate::arch_disable_interrupts();
    }

    SERIAL1
        .lock()
        .write_fmt(args)
        .expect("Printing to serial failed");

    // Only re-enable if they were enabled before
    // This prevents race condition in syscall handler where interrupts must stay disabled
    if irq_enabled {
        unsafe {
            crate::arch_enable_interrupts();
        }
    }
}

/// Try to print without blocking - returns Err if lock is held
pub fn try_print(args: fmt::Arguments) -> Result<(), ()> {
    use core::fmt::Write;

    // CRITICAL: Check if interrupts are currently enabled
    // We must NOT re-enable interrupts if they were disabled by syscall entry
    let irq_enabled = crate::arch_interrupts_enabled();

    // Disable interrupts while holding the lock
    unsafe {
        crate::arch_disable_interrupts();
    }

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
        unsafe {
            crate::arch_enable_interrupts();
        }
    }

    result
}

/// Write `s` to COM1 as one unit, from a path that must not wait indefinitely
/// on a lock (an interrupt return, the syscall entry). It spins, with
/// interrupts masked, for up to 2^20 `try_lock` attempts on SERIAL1, so the
/// line is not interleaved byte by byte with another CPU's output, and writes
/// straight to the port if SERIAL1 stays busy.
///
/// This is a bounded wait for a lock, and its callers are once-per-boot
/// markers. SERIAL1 is held with interrupts masked (`_print`), so a holder on
/// another CPU finishes its line and releases it; a holder on this CPU cannot
/// be waited for, which the bound covers.
pub fn write_str_bounded(s: &str) {
    use core::fmt::Write;
    const ATTEMPTS: u32 = 1 << 20;

    crate::arch_without_interrupts(|| {
        for _ in 0..ATTEMPTS {
            if let Some(mut serial) = SERIAL1.try_lock() {
                let _ = serial.write_str(s);
                return;
            }
            core::hint::spin_loop();
        }
        // SAFETY: COM1's data port; the bytes may interleave with the holder's.
        unsafe {
            let mut port = x86_64::instructions::port::Port::<u8>::new(COM1_PORT);
            for byte in s.bytes() {
                port.write(byte);
            }
        }
    });
}

/// Emergency print for panics - uses direct port I/O without locking
/// WARNING: May corrupt output if racing with normal serial output
#[allow(dead_code)]
pub fn emergency_print(args: fmt::Arguments) -> Result<(), ()> {
    use core::fmt::Write;

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

    crate::arch_without_interrupts(|| {
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
///
/// The line goes into the log ring whole, then `drain_log` writes it out
/// unless another context is already writing the ring.
#[doc(hidden)]
pub fn _log_print(args: fmt::Arguments) {
    use core::fmt::Write;

    crate::arch_without_interrupts(|| {
        let mut ring = LOG_RING.lock();
        if ring.dropped != 0 {
            let dropped = core::mem::take(&mut ring.dropped);
            let _ = write!(RingWriter(&mut ring), "[log: {} bytes dropped]\n", dropped);
        }
        let _ = RingWriter(&mut ring).write_fmt(args);
    });
    drain_log();
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
