#![cfg(target_arch = "x86_64")]

use conquer_once::spin::OnceCell;
use core::fmt;
use crossbeam_queue::ArrayQueue;
use futures_util::task::AtomicWaker;
use spin::Mutex;
use uart_16550::SerialPort;

pub use crate::serial_writer::Writer;

use crate::log_ring::LogRing;

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
/// the copy, and go out one FIFO's worth at a time (`write_burst`), written
/// by whichever context finds the UART ready. Only a caller that may wait
/// (`_log_print`) waits for the UART between bursts.
const LOG_RING_BYTES: usize = 64 * 1024;

/// Which lines the ring takes and which it drops: `log_ring`. A line a
/// boot-path stage depends on is never dropped.
static LOG_RING: Mutex<LogRing<LOG_RING_BYTES>> = Mutex::new(LogRing::new());

/// Held by the one context taking a burst from the ring and writing it to
/// COM2, so bursts reach the UART in the order they left the ring. It is held
/// with interrupts masked and never across a UART wait, so its holder cannot
/// be switched out, or have its continuation discarded, while it holds it.
static LOG_PRINTER: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Ring bytes written to COM2 so far, counted under `LOG_PRINTER` once the
/// burst is in the UART.
static LOG_SENT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// Set once an idle loop drains the log (`drain_log_while`). Until then no
/// context exists to write out what a caller that cannot wait leaves in the
/// ring, so every caller waits on the UART for its own line, as the boot
/// CPU's early, interrupts-masked initialisation does.
static LOG_IDLE_DRAIN: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

fn log_burst() -> usize {
    if SERIAL2_FIFO.load(core::sync::atomic::Ordering::Relaxed) { 16 } else { 1 }
}

fn log_uart_status() -> u8 {
    // SAFETY: COM2's line status register.
    unsafe { x86_64::instructions::port::Port::<u8>::new(COM2_PORT + 5).read() }
}

/// Write `bytes`, at most one FIFO, to COM2's transmitter, which is empty.
fn write_log_uart(bytes: &[u8]) {
    let mut data = x86_64::instructions::port::Port::<u8>::new(COM2_PORT);
    for &byte in bytes {
        // SAFETY: COM2's data port; only the `LOG_PRINTER` holder writes COM2.
        unsafe { data.write(byte) };
    }
}

enum Burst {
    /// The ring is empty.
    Empty,
    /// One burst went to the UART.
    Wrote,
    /// Another context holds the printer role, or the UART has not taken the
    /// last burst yet.
    Busy,
}

/// Write one FIFO's worth of the ring to COM2 if the UART can take it now.
/// Never waits: the printer role is held, with interrupts masked, for one
/// status read, the take from the ring and at most 16 port writes.
fn write_burst() -> Burst {
    use core::sync::atomic::Ordering;
    crate::arch_without_interrupts(|| {
        if LOG_PRINTER
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            return Burst::Busy;
        }
        let burst = if log_uart_status() & 0x20 == 0 {
            if LOG_RING.lock().len() == 0 { Burst::Empty } else { Burst::Busy }
        } else {
            let mut chunk = [0u8; 16];
            let n = LOG_RING.lock().pop(&mut chunk[..log_burst()]);
            write_log_uart(&chunk[..n]);
            LOG_SENT.fetch_add(n as u64, Ordering::Release);
            if n == 0 { Burst::Empty } else { Burst::Wrote }
        };
        LOG_PRINTER.store(false, Ordering::Release);
        burst
    })
}

/// Write the ring out until `target` bytes have gone to COM2 in all, waiting
/// on the UART between bursts with interrupts as the caller has them.
fn drain_until(target: u64) {
    use core::sync::atomic::Ordering;
    while LOG_SENT.load(Ordering::Acquire) < target {
        match write_burst() {
            Burst::Empty => return,
            Burst::Wrote => {}
            Burst::Busy => core::hint::spin_loop(),
        }
    }
}

/// Write the log ring out while `go_on` holds, from a CPU's idle loop: what a
/// caller that could not wait on the UART left in the ring goes out here.
/// Each burst masks interrupts only for its own port writes, and the idle
/// thread can be switched out between bursts as anywhere else in the loop.
pub fn drain_log_while(go_on: impl Fn() -> bool) {
    LOG_IDLE_DRAIN.store(true, core::sync::atomic::Ordering::Release);
    while go_on() {
        match write_burst() {
            Burst::Empty => return,
            Burst::Wrote => {}
            Burst::Busy => core::hint::spin_loop(),
        }
    }
}

/// Write out whatever the kernel log still holds, from a context about to
/// stop the machine: a panic, or a debug-exit write that ends QEMU, after
/// which a line still in the ring is lost. It takes the printer role first, so
/// a burst another CPU has taken from the ring reaches the UART before what
/// follows it, and waits for the transmitter to empty before it returns.
/// Bounded: a role or ring lock that stays held, as one this CPU was stopped
/// holding would, is passed over.
pub fn flush_log() {
    use core::sync::atomic::Ordering;
    const ATTEMPTS: u32 = 1 << 20;
    crate::arch_without_interrupts(|| {
        let mut printer = false;
        for _ in 0..ATTEMPTS {
            if LOG_PRINTER
                .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
            {
                printer = true;
                break;
            }
            core::hint::spin_loop();
        }
        for _ in 0..ATTEMPTS {
            if let Some(mut ring) = LOG_RING.try_lock() {
                let mut chunk = [0u8; 16];
                loop {
                    let n = ring.pop(&mut chunk[..log_burst()]);
                    if n == 0 {
                        break;
                    }
                    while log_uart_status() & 0x20 == 0 {
                        core::hint::spin_loop();
                    }
                    write_log_uart(&chunk[..n]);
                    LOG_SENT.fetch_add(n as u64, Ordering::Release);
                }
                break;
            }
            core::hint::spin_loop();
        }
        // The transmitter empty bit: the last burst has left the UART.
        for _ in 0..ATTEMPTS {
            if log_uart_status() & 0x40 != 0 {
                break;
            }
            core::hint::spin_loop();
        }
        if printer {
            LOG_PRINTER.store(false, Ordering::Release);
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

pub fn write_byte(writer: Writer, byte: u8) {
    write_bytes_atomic(writer, core::slice::from_ref(&byte));
}

/// Write one caller-defined output unit from `writer` while holding the serial
/// lock once, starting a fresh line first if another writer's line is open
/// (`serial_writer`).
pub fn write_bytes_atomic(writer: Writer, bytes: &[u8]) {
    // CRITICAL: Check if interrupts are currently enabled
    // We must NOT re-enable interrupts if they were disabled by syscall entry
    let irq_enabled = crate::arch_interrupts_enabled();

    // Disable interrupts while holding the lock
    unsafe {
        crate::arch_disable_interrupts();
    }

    let mut serial = SERIAL1.lock();
    let lead = crate::serial_writer::lead(writer, bytes);
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
            for &byte in lead.iter().chain(bytes) {
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
        for &byte in lead.iter().chain(bytes) {
            serial.send(byte);
            crate::log_buffer::capture_byte(byte);
        }
    }
    if let Some(&last) = bytes.last() {
        crate::serial_writer::wrote(writer, last);
    }
    drop(serial);

    // Only re-enable if they were enabled before
    // This prevents race condition in syscall handler where interrupts must stay disabled
    if irq_enabled {
        unsafe {
            crate::arch_enable_interrupts();
        }
    }
}

/// One unit from `writer` written to COM1, which its caller holds: it starts a
/// fresh line first if another writer's line is open (`serial_writer`).
struct Unit<'a> {
    serial: &'a mut SerialPort,
    writer: Writer,
}

impl fmt::Write for Unit<'_> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let Some(&last) = s.as_bytes().last() else {
            return Ok(());
        };
        for &byte in crate::serial_writer::lead(self.writer, s.as_bytes()) {
            self.serial.send(byte);
        }
        self.serial.write_str(s)?;
        crate::serial_writer::wrote(self.writer, last);
        Ok(())
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

    Unit {
        serial: &mut SERIAL1.lock(),
        writer: Writer::Kernel,
    }
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
            Unit {
                serial: &mut serial,
                writer: Writer::Kernel,
            }
            .write_fmt(args)
            .map_err(|_| ())?;
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
                let _ = Unit {
                    serial: &mut serial,
                    writer: Writer::Kernel,
                }
                .write_str(s);
                return;
            }
            core::hint::spin_loop();
        }
        // SAFETY: COM1's data port; the bytes may interleave with the holder's.
        // Without SERIAL1 this neither reads nor updates `serial_writer`'s line state.
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
/// The line goes into the log ring whole. A caller that may wait on the UART,
/// one that can be preempted, then writes the ring out up to the end of its
/// own line. A caller that cannot, holding a lock or with interrupts masked,
/// writes at most the one burst the UART can take at once and leaves the
/// rest to the next caller that may wait or to an idle CPU
/// (`drain_log_while`). Until an idle loop drains the log every caller waits.
#[doc(hidden)]
pub fn _log_print(args: fmt::Arguments) {
    use core::sync::atomic::Ordering;

    let may_wait = !LOG_IDLE_DRAIN.load(Ordering::Acquire)
        || (crate::per_cpu::is_initialized()
            && crate::arch_interrupts_enabled()
            && crate::per_cpu::preempt_count() == 0);
    if may_wait {
        // Make room first, so a backlog does not drop this line.
        let backlog = crate::arch_without_interrupts(|| {
            let ring = LOG_RING.lock();
            (ring.len() > LOG_RING_BYTES / 2).then_some(ring.pushed())
        });
        if let Some(target) = backlog {
            drain_until(target);
        }
    }
    let end = enqueue_log(args);
    if may_wait {
        drain_until(end);
    } else {
        let _ = write_burst();
    }
}

/// Append a COM2 line without waiting for the UART: an ordinary line that
/// does not fit is dropped whole, and the caller arranges delivery via the
/// drainers. Interrupts are masked while formatting into the fixed ring.
///
/// A line a boot-path stage depends on is never dropped (`log_ring`): when
/// even the ring's reserve cannot take it, this writes the ring out until it
/// can, waiting on the UART with interrupts as the caller has them. The
/// reserve makes that a last resort.
pub(crate) fn enqueue_log(args: fmt::Arguments) -> u64 {
    loop {
        let appended =
            crate::arch_without_interrupts(|| LOG_RING.lock().append(args, crate::STAGE_MARKERS));
        match appended {
            Ok(end) => return end,
            Err(needed) => make_room(needed),
        }
    }
}

/// Write the ring out until it has `needed` free bytes.
fn make_room(needed: usize) {
    while crate::arch_without_interrupts(|| LOG_RING_BYTES - LOG_RING.lock().len() < needed) {
        match write_burst() {
            Burst::Empty | Burst::Wrote => {}
            Burst::Busy => core::hint::spin_loop(),
        }
    }
}

/// Try one FIFO burst without polling UART readiness, then report whether
/// bytes remain. Thread-context housekeeping sleeps between calls.
pub(crate) fn drain_log_once() -> bool {
    let _ = write_burst();
    crate::arch_without_interrupts(|| LOG_RING.lock().len() != 0)
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
