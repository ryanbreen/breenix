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

/// COM1 (0x3F8): the console. Its lock covers the port and the output queued
/// for it (`TxQueue`).
pub static SERIAL1: Mutex<Com1> = Mutex::new(Com1 {
    port: unsafe { SerialPort::new(COM1_PORT) },
    queue: TxQueue::new(),
});

pub struct Com1 {
    port: SerialPort,
    queue: TxQueue,
}

/// COM2 (0x2F8) - Used exclusively for kernel log output.
/// This separates kernel logs from user I/O (COM1), enabling interactive shell use.
pub static SERIAL2: Mutex<SerialPort> = Mutex::new(unsafe { SerialPort::new(COM2_PORT) });

// Serial input queue and waker (similar to keyboard)
static SERIAL_INPUT_QUEUE: OnceCell<ArrayQueue<u8>> = OnceCell::uninit();
static SERIAL_WAKER: AtomicWaker = AtomicWaker::new();

static SERIAL1_FIFO: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
static SERIAL2_FIFO: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// COM1 output accepted from its writers and not yet given to the UART, kept
/// under SERIAL1 with the port.
///
/// A writer used to give the UART its own output, a port write a byte and a
/// status read a burst, each a trap to the emulated UART. Under the x86 gate's
/// virtualization one access takes about 75 us, so one 75-byte suite record
/// kept its writer in write(2) for 6 ms and more, with interrupts masked and
/// charged to the writing thread's CPU clock (#1415). Output now goes into this
/// queue under SERIAL1, and the COM1 transmit thread (`com1_transmitter`) gives
/// it to the UART one byte at a time, with interrupts masked for that byte
/// only. When the UART's FIFO is full the thread enables the
/// transmitter-empty interrupt and blocks, and that interrupt wakes it again
/// (`transmitter_interrupt`). A writer with output for an idle thread starts
/// it the same way, with one port write. A writer gives the UART its output
/// itself only when the queue is full, and before the thread exists
/// (`start_transmitter`).
const TX_QUEUE_BYTES: usize = 16 * 1024;

struct TxQueue {
    bytes: [u8; TX_QUEUE_BYTES],
    head: usize,
    len: usize,
    /// Bytes the UART's FIFO can still take without a status read: set to the
    /// FIFO's depth when the status register shows the transmitter empty, and
    /// counted down by every byte written under SERIAL1, so writers taking
    /// turns byte by byte never overrun it.
    room: usize,
}

impl TxQueue {
    const fn new() -> Self {
        Self { bytes: [0; TX_QUEUE_BYTES], head: 0, len: 0, room: 0 }
    }

    fn push(&mut self, byte: u8) -> bool {
        if self.len == TX_QUEUE_BYTES {
            return false;
        }
        self.bytes[(self.head + self.len) % TX_QUEUE_BYTES] = byte;
        self.len += 1;
        true
    }

    fn pop(&mut self, out: &mut [u8]) -> usize {
        let n = out.len().min(self.len);
        for (i, slot) in out[..n].iter_mut().enumerate() {
            *slot = self.bytes[(self.head + i) % TX_QUEUE_BYTES];
        }
        self.head = (self.head + n) % TX_QUEUE_BYTES;
        self.len -= n;
        n
    }
}

/// The COM1 transmit thread sends queued output (`start_transmitter`).
static TX_THREAD: spin::Once<crate::task::kthread::KthreadHandle> = spin::Once::new();

/// Where the transmit thread is. IDLE: blocked with nothing to send, the
/// transmitter-empty interrupt disabled. ARMED: blocked, the interrupt enabled
/// to wake it. RUNNING: woken, sending. Writers move IDLE to ARMED and the
/// interrupt ARMED to RUNNING. Only the thread leaves RUNNING, under SERIAL1.
static TX_STATE: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(TX_IDLE);
const TX_IDLE: u8 = 0;
const TX_ARMED: u8 = 1;
const TX_RUNNING: u8 = 2;

const IER_RECEIVED: u8 = 0x01;
const IER_TRANSMITTER_EMPTY: u8 = 0x02;
const LSR_TRANSMITTER_EMPTY: u8 = 0x20;
const LSR_TRANSMITTER_IDLE: u8 = 0x40;

fn com1_burst() -> usize {
    if SERIAL1_FIFO.load(core::sync::atomic::Ordering::Relaxed) { 16 } else { 1 }
}

fn com1_status() -> u8 {
    // SAFETY: COM1's line status register.
    unsafe { x86_64::instructions::port::Port::<u8>::new(COM1_PORT + 5).read() }
}

fn write_com1_data(bytes: &[u8]) {
    let mut data = x86_64::instructions::port::Port::<u8>::new(COM1_PORT);
    for &byte in bytes {
        // SAFETY: COM1's data port, written with SERIAL1 held.
        unsafe { data.write(byte) };
    }
}

fn write_com1_ier(armed: bool) {
    let ier = IER_RECEIVED | if armed { IER_TRANSMITTER_EMPTY } else { 0 };
    // SAFETY: COM1's interrupt enable register, written with SERIAL1 held or,
    // disarming only, from its interrupt (`transmitter_interrupt`).
    unsafe { x86_64::instructions::port::Port::<u8>::new(COM1_PORT + 1).write(ier) };
}

/// Make room in the UART's FIFO if the status register shows the transmitter
/// empty. Called with SERIAL1 held.
fn refill_room(queue: &mut TxQueue) -> bool {
    if queue.room == 0 && com1_status() & LSR_TRANSMITTER_EMPTY != 0 {
        queue.room = com1_burst();
    }
    queue.room != 0
}

/// Give the UART the next FIFO's worth of `queue`, waiting for room. Called
/// with SERIAL1 held and interrupts masked.
fn write_com1_burst(queue: &mut TxQueue) {
    while !refill_room(queue) {
        core::hint::spin_loop();
    }
    let mut chunk = [0u8; 16];
    let n = queue.pop(&mut chunk[..queue.room]);
    write_com1_data(&chunk[..n]);
    queue.room -= n;
}

enum Sent {
    /// One byte went to the UART.
    Byte,
    /// The queue is empty.
    Empty,
    /// The UART's FIFO is full.
    Full,
}

/// Give the UART the next queued byte if its FIFO has room. Never waits: one
/// data write, and a status read once per FIFO. Called with SERIAL1 held and
/// interrupts masked.
fn send_byte(queue: &mut TxQueue) -> Sent {
    if queue.len == 0 {
        return Sent::Empty;
    }
    if !refill_room(queue) {
        return Sent::Full;
    }
    let mut byte = [0u8];
    queue.pop(&mut byte);
    write_com1_data(&byte);
    queue.room -= 1;
    Sent::Byte
}

/// Write `queue` out, waiting on the UART between bursts. Called with SERIAL1
/// held and interrupts masked.
fn drain_com1(queue: &mut TxQueue) {
    while queue.len != 0 {
        write_com1_burst(queue);
    }
}

/// Queue one byte for COM1, translating backspace and DEL to BS SPACE BS as
/// `uart_16550::SerialPort::send` does. A full queue is written out to make
/// room, waiting on the UART. Called with SERIAL1 held and interrupts masked.
fn queue_byte(queue: &mut TxQueue, byte: u8) {
    let wire: &[u8] = if byte == 0x08 || byte == 0x7f { &[0x08, b' ', 0x08] } else { core::slice::from_ref(&byte) };
    for &transmitted in wire {
        while !queue.push(transmitted) {
            write_com1_burst(queue);
        }
    }
}

/// Hand what was just queued on: to the transmit thread once it exists, with
/// at most one port write, else to the UART now. Called with SERIAL1 held and
/// interrupts masked.
fn start_queued(queue: &mut TxQueue) {
    use core::sync::atomic::Ordering;
    if TX_THREAD.get().is_none() {
        drain_com1(queue);
    } else if queue.len != 0 && TX_STATE.load(Ordering::Acquire) == TX_IDLE {
        TX_STATE.store(TX_ARMED, Ordering::Release);
        write_com1_ier(true);
    }
}

/// COM1's interrupt, transmitter side: wake the transmit thread. One port
/// write, disabling the transmitter-empty interrupt so its request line drops
/// and receive interrupts still arrive, and only when the thread is waiting
/// for that interrupt. Takes no lock: the wake goes into the scheduler's
/// interrupt wake buffer (`wake_thread_any_context`).
pub fn transmitter_interrupt() {
    use core::sync::atomic::Ordering;
    if TX_STATE.load(Ordering::Acquire) != TX_ARMED {
        return;
    }
    write_com1_ier(false);
    if TX_STATE
        .compare_exchange(TX_ARMED, TX_RUNNING, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
    {
        if let Some(thread) = TX_THREAD.get() {
            crate::task::scheduler::wake_thread_any_context(thread.tid());
        }
    }
}

/// Block the transmit thread until the transmitter-empty interrupt hands it
/// output (TX_RUNNING). The state is checked under the scheduler lock, which
/// the interrupt wake buffer is drained under, so a wake sent after the check
/// finds the thread blocked.
fn wait_for_output() {
    use core::sync::atomic::Ordering;
    let running = || TX_STATE.load(Ordering::Acquire) == TX_RUNNING;
    while !running() {
        crate::arch_without_interrupts(|| {
            crate::task::scheduler::with_scheduler(|sched| {
                if !running() {
                    sched.block_current();
                }
            });
        });
        crate::task::waitqueue::halt_blocked_current();
    }
}

/// The COM1 transmit thread: give the UART queued output one byte at a time,
/// each with interrupts masked for its one data write, until the queue is
/// empty or the UART's FIFO is full, then block until the transmitter-empty
/// interrupt wakes it.
fn com1_transmitter() {
    use core::sync::atomic::Ordering;
    loop {
        wait_for_output();
        let mut sending = true;
        while sending {
            sending = crate::arch_without_interrupts(|| {
                let mut serial = SERIAL1.lock();
                match send_byte(&mut serial.queue) {
                    Sent::Byte => true,
                    Sent::Empty => {
                        TX_STATE.store(TX_IDLE, Ordering::Release);
                        false
                    }
                    Sent::Full => {
                        TX_STATE.store(TX_ARMED, Ordering::Release);
                        write_com1_ier(true);
                        false
                    }
                }
            });
        }
    }
}

/// Start the COM1 transmit thread; from here writers queue their output for
/// it rather than give it to the UART themselves. Called once, when kernel
/// threads can run.
pub fn start_transmitter() {
    TX_THREAD.call_once(|| {
        crate::task::kthread::kthread_run(com1_transmitter, "kcom1tx")
            .expect("could not spawn the COM1 transmit thread")
    });
}

/// Wait until the COM1 output queued so far has left the UART, for a caller
/// that must not return before it has (TCSETSW, TCSETSF). It gives the UART
/// the queue itself, a byte at a time with interrupts masked for each, and
/// waits for room with interrupts as the caller has them.
pub fn drain_output() {
    let mut done = false;
    while !done {
        done = crate::arch_without_interrupts(|| {
            let mut serial = SERIAL1.lock();
            match send_byte(&mut serial.queue) {
                Sent::Byte | Sent::Full => false,
                Sent::Empty => true,
            }
        });
        if !done {
            core::hint::spin_loop();
        }
    }
    while com1_status() & LSR_TRANSMITTER_IDLE == 0 {
        core::hint::spin_loop();
    }
}

/// Write out the COM1 output still queued, from a context about to stop the
/// machine (`flush_log`), and wait for the transmitter to empty. Bounded like
/// `flush_log`: a SERIAL1 that stays held is passed over.
fn flush_com1(attempts: u32) {
    for _ in 0..attempts {
        if let Some(mut serial) = SERIAL1.try_lock() {
            drain_com1(&mut serial.queue);
            drop(serial);
            break;
        }
        core::hint::spin_loop();
    }
    for _ in 0..attempts {
        if com1_status() & LSR_TRANSMITTER_IDLE != 0 {
            break;
        }
        core::hint::spin_loop();
    }
}

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
    write_notices();
    while go_on() {
        match write_burst() {
            Burst::Empty => return,
            Burst::Wrote => {}
            Burst::Busy => core::hint::spin_loop(),
        }
    }
}

/// Write out whatever COM1's queue and the kernel log still hold, from a
/// context about to stop the machine: a panic, or a debug-exit write that ends
/// QEMU, after which a line still queued is lost. COM1 goes first
/// (`flush_com1`). For the log it takes the printer role first, so a burst
/// another CPU has taken from the ring reaches the UART before what follows
/// it, and waits for the transmitter to empty before it returns. Bounded: a
/// role or lock that stays held, as one this CPU was stopped holding would, is
/// passed over.
pub fn flush_log() {
    use core::sync::atomic::Ordering;
    const ATTEMPTS: u32 = 1 << 20;
    crate::arch_without_interrupts(|| {
        flush_com1(ATTEMPTS);
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
    init_port(&mut SERIAL1.lock().port, COM1_PORT);
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

/// Use the PC console's 115200 baud rather than the library's 38400 default,
/// which takes over 20 ms to send an ordinary 80-byte line.
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

    // Enable receive interrupts (when data is available).
    crate::arch_without_interrupts(|| {
        let serial = SERIAL1.lock();
        write_com1_ier(false);
        drop(serial);
    });

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

/// Queue one caller-defined output unit from `writer` for COM1 while holding
/// the serial lock once, starting a fresh line first if another writer's line
/// is open (`serial_writer`).
pub fn write_bytes_atomic(writer: Writer, bytes: &[u8]) {
    // CRITICAL: Check if interrupts are currently enabled
    // We must NOT re-enable interrupts if they were disabled by syscall entry
    let irq_enabled = crate::arch_interrupts_enabled();

    // Disable interrupts while holding the lock
    unsafe {
        crate::arch_disable_interrupts();
    }

    let mut serial = SERIAL1.lock();
    let queue = &mut serial.queue;
    let lead = crate::serial_writer::lead(writer, bytes);
    for &byte in lead.iter().chain(bytes) {
        queue_byte(queue, byte);
        crate::log_buffer::capture_byte(byte);
    }
    if let Some(&last) = bytes.last() {
        crate::serial_writer::wrote(writer, last);
    }
    start_queued(queue);
    drop(serial);

    // Only re-enable if they were enabled before
    // This prevents race condition in syscall handler where interrupts must stay disabled
    if irq_enabled {
        unsafe {
            crate::arch_enable_interrupts();
        }
    }
}

/// One unit from `writer` queued for COM1, whose SERIAL1 its caller holds: it
/// starts a fresh line first if another writer's line is open
/// (`serial_writer`). The caller hands the queue on (`start_queued`).
struct Unit<'a> {
    queue: &'a mut TxQueue,
    writer: Writer,
}

impl fmt::Write for Unit<'_> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let Some(&last) = s.as_bytes().last() else {
            return Ok(());
        };
        for &byte in crate::serial_writer::lead(self.writer, s.as_bytes()).iter().chain(s.as_bytes()) {
            queue_byte(self.queue, byte);
        }
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

    let mut serial = SERIAL1.lock();
    Unit {
        queue: &mut serial.queue,
        writer: Writer::Kernel,
    }
    .write_fmt(args)
    .expect("Printing to serial failed");
    start_queued(&mut serial.queue);
    drop(serial);

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
            let written = Unit {
                queue: &mut serial.queue,
                writer: Writer::Kernel,
            }
            .write_fmt(args)
            .map_err(|_| ());
            start_queued(&mut serial.queue);
            drop(serial);
            written
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
/// line is not interleaved byte by byte with another CPU's output.
///
/// If SERIAL1 stays busy, `s` goes to the kernel log instead, whole, as a
/// lock-free notice (`log_notice::write_or_post`) written out from thread
/// context. Written to the port without SERIAL1, its bytes would land inside
/// the holder's unit, which may be a line a boot-path stage depends on. Half
/// the notices are kept for boot-path stage lines (`stage_lines`).
pub fn write_str_bounded(s: &str) {
    use crate::log_notice::Node;
    use core::fmt::Write;
    const ATTEMPTS: u32 = 1 << 20;
    static FALLBACK: [Node; 4] = [const { Node::new() }; 4];

    let stage = crate::stage_lines::contains(crate::STAGE_MARKERS, s.as_bytes());
    let line = s.strip_suffix('\n').unwrap_or(s);
    crate::arch_without_interrupts(|| {
        crate::log_notice::write_or_post(&FALLBACK, line, stage, ATTEMPTS, || {
            let Some(mut serial) = SERIAL1.try_lock() else {
                return false;
            };
            let _ = Unit {
                queue: &mut serial.queue,
                writer: Writer::Kernel,
            }
            .write_str(s);
            start_queued(&mut serial.queue);
            drop(serial);
            true
        })
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

    write_notices_from_thread();

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

/// Append the lines posted on paths that may not log (`log_notice`) to the
/// kernel log, oldest first, with interrupts masked until every one taken is in.
pub fn write_notices() {
    if !crate::log_notice::pending() {
        return;
    }
    crate::arch_without_interrupts(|| {
        crate::log_notice::take(|line| {
            let text = core::str::from_utf8(line).unwrap_or("[log: notice is not UTF-8]\n");
            enqueue_log(format_args!("{}", text));
        })
    });
}

/// [`write_notices`], from thread context only: never from an interrupt or
/// softirq, nor with interrupts masked.
fn write_notices_from_thread() {
    if crate::log_notice::pending()
        && crate::per_cpu::is_initialized()
        && crate::arch_interrupts_enabled()
        && !crate::per_cpu::in_interrupt()
    {
        write_notices();
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
