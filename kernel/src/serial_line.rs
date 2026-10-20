//! Aarch64 UART ownership (issue 847).
//!
//! Every aarch64 UART writer goes through this module. A record (a line, or a
//! caller-defined unterminated unit such as a TTY character or a breadcrumb)
//! is assembled on the caller's stack, then written to the UART in one piece
//! while this CPU owns the UART (`uart_owner`, which has the ownership rules).
//!
//! Kernel records are synchronous: a kernel record reaches the UART before
//! the call that completes it returns. A record longer than the assembly
//! buffer is written in buffer-sized pieces, each one owned. The one writer
//! that may discard its record is [`try_write`], for breadcrumbs on paths that
//! must not wait. Terminal output is queued for the transmit thread once it
//! runs (`TxRing`), and a kernel record first writes out the terminal output
//! queued before it, so the UART still carries the records in the order they
//! were written.
//!
//! Lines posted on paths that may not log (`log_notice`) are written here from
//! thread context only: by the next logged record a thread writes, and by the idle
//! loop ([`write_notices_from_thread`]). Each is a whole line of its own.
use crate::serial_writer::Writer;
use crate::uart_owner::{self, Kind, Port};
use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};

/// Assembly buffer size. Records up to this length are written in one owned piece.
const CAPACITY: usize = 1024;

/// The aarch64 UART, as `uart_owner` drives it.
pub(crate) enum Uart {}

/// Proof that this CPU owns the UART. Only `uart_owner` creates one.
pub(crate) type Ownership = uart_owner::Ownership<Uart>;

static STATE: uart_owner::State = uart_owner::State::new();

impl Port for Uart {
    type Writer = Writer;

    fn state() -> &'static uart_owner::State {
        &STATE
    }

    /// From MPIDR, so it is valid before per-CPU state exists.
    #[inline(always)]
    fn cpu_tag() -> u32 {
        let mpidr: u64;
        unsafe {
            core::arch::asm!("mrs {}, mpidr_el1", out(reg) mpidr, options(nomem, nostack, preserves_flags));
        }
        // Aff3 goes in the top byte, above Aff2..Aff0.
        (((mpidr >> 8) & 0xFF00_0000) | (mpidr & 0x00FF_FFFF)) as u32 + 1
    }

    #[inline(always)]
    fn counter() -> u64 {
        let ticks: u64;
        unsafe {
            core::arch::asm!("isb", "mrs {}, cntvct_el0", out(reg) ticks, options(nomem, nostack, preserves_flags));
        }
        ticks
    }

    /// Divides only by a constant: the soft-lockup report reaches this through
    /// `Ownership::acquire`, and dividing by `ns` links a panic into it
    /// whenever this is not inlined (`scripts/check-aarch64-lockup-no-alloc.sh`).
    fn ticks(ns: u64) -> u64 {
        let frequency: u64;
        unsafe {
            core::arch::asm!("mrs {}, cntfrq_el0", out(reg) frequency, options(nomem, nostack, preserves_flags));
        }
        // A zero CNTFRQ (misconfigured firmware) still gets a finite wait.
        frequency.max(1_000_000).saturating_mul(ns) / 1_000_000_000
    }

    /// Masks IRQs and FIQs.
    fn mask_interrupts() -> u64 {
        let daif: u64;
        unsafe {
            core::arch::asm!("mrs {}, daif", out(reg) daif, options(nomem, nostack, preserves_flags));
            core::arch::asm!("msr daifset, #0x3", options(nomem, nostack, preserves_flags));
        }
        crate::arch_impl::aarch64::irq_mask_account::opened();
        daif
    }

    fn restore_interrupts(daif: u64) {
        crate::arch_impl::aarch64::irq_mask_account::restoring(daif);
        unsafe {
            core::arch::asm!("msr daif, {}", in(reg) daif, options(nomem, nostack, preserves_flags));
        }
    }

    fn ready() -> bool {
        crate::serial_aarch64::hardware_ready()
    }

    fn write(byte: u8, owner: &Ownership) {
        crate::serial_aarch64::hardware_write(byte, owner);
    }

    fn capture(byte: u8) {
        crate::log_buffer::capture_byte(byte);
    }

    fn lead(writer: Writer, unit: &[u8]) -> &'static [u8] {
        crate::serial_writer::lead(writer, unit)
    }

    fn line_open() -> bool {
        crate::serial_writer::line_open()
    }

    fn wrote(writer: Writer, byte: u8) {
        crate::serial_writer::wrote(writer, byte);
    }

    fn markers() -> &'static [&'static str] {
        crate::STAGE_MARKERS
    }
}

/// Capture a formatted record in the kernel log only, under the same
/// exclusion as UART log capture. The formatter feeds capture directly.
pub fn capture_fmt(args: core::fmt::Arguments) {
    uart_owner::capture_only::<Uart>(args);
}

/// Write one record from `writer` to the UART while owning it, on a fresh
/// line if another writer's line is open. This is the accounting boundary of
/// kernel output (`output_accounting`): the record's bytes count once, toward
/// the current thread when `attributed`, and the lead newline does not count.
fn emit<const TEE: bool, const TERMINAL: bool>(writer: Writer, bytes: &[u8], level: log::Level, attributed: bool) {
    if bytes.is_empty() {
        return;
    }
    if TERMINAL && writer == Writer::Tty && queue_terminal(bytes) {
        return;
    }
    if writer == Writer::Kernel {
        crate::output_accounting::count(bytes.len(), attributed);
    }
    // Terminal output queued before this record goes first, from thread context:
    // raw kernel records (the timer's lockup capture among them) and interrupt
    // handlers skip it, as they skip the notices.
    if (TEE || TERMINAL) && in_thread_context() {
        write_queued();
    }
    // Raw diagnostics (including the timer's lockup capture) must stay on
    // their fixed-width writer path. Notice draining tees into the log rings
    // and can reach bounds-panic formatting, even though its runtime context
    // check refuses to drain from an IRQ. Keep that call out of raw records'
    // linked call graph; logged records and the idle loop drain the list.
    if TEE {
        write_notices_from_thread();
    }
    uart_owner::write::<Uart, TEE>(writer, bytes, Kind::Plain, &mut Ownership::acquire());
    // The kernel log, as the log rings keep it, also goes to the screen console.
    if TEE && writer == Writer::Kernel {
        crate::console::log_bytes_at(level, bytes);
    }
}

/// Thread context with interrupts enabled: not an interrupt or softirq handler.
fn in_thread_context() -> bool {
    crate::per_cpu::is_initialized() && crate::arch_interrupts_enabled() && !crate::per_cpu::in_interrupt()
}

/// Write out the lines posted on paths that may not log (`log_notice`), oldest
/// first, each a whole line of its own. From any context: `Notice::drop` needs
/// the list written out wherever it runs. Output paths call
/// [`write_notices_from_thread`] instead.
pub fn write_notices() {
    if crate::log_notice::pending() {
        let mut ownership = Ownership::acquire();
        crate::log_notice::take(|line| {
            uart_owner::write::<Uart, true>(Writer::Kernel, line, Kind::Line, &mut ownership);
            crate::console::log_bytes_at(log::Level::Warn, line);
        });
    }
}

/// Posted lines were counted when they were posted: writing them out here is a
/// drain, not counted again.
///
/// [`write_notices`], from thread context only: never from an interrupt or
/// softirq, nor with interrupts masked, so interrupt paths and the timer's
/// lockup report never write the list.
pub fn write_notices_from_thread() {
    if crate::log_notice::pending() && in_thread_context() {
        write_notices();
    }
}

/// Write `bytes` as one record if the UART can be owned without waiting, for
/// breadcrumbs on paths that must not wait on another CPU's output. Returns
/// false, having written nothing, when another CPU owns or is waiting for it.
pub fn try_write(bytes: &[u8]) -> bool {
    match Ownership::try_acquire() {
        Some(mut ownership) => {
            crate::output_accounting::count(bytes.len(), true);
            uart_owner::write::<Uart, false>(Writer::Kernel, bytes, Kind::Plain, &mut ownership);
            true
        }
        None => false,
    }
}

// =============================================================================
// Terminal output queue
// =============================================================================

/// Terminal output (`Writer::Tty`) accepted from its writers and not yet given
/// to the UART. A writer used to give the UART its own output, two traps to the
/// emulated UART per byte with interrupts masked, so one 70-byte suite record
/// kept its writer in write(2) for about 600 us on ARM64 QEMU, charged to the
/// writing thread's CPU clock, and longer by however long the host held the
/// emulated UART meanwhile (#2083; x86's COM1 queue was added for the same
/// cause, #1415). Once the
/// transmit thread runs, a terminal record that fits goes into this ring and
/// the thread gives it to the UART (`terminal_transmitter`), each record whole
/// under one ownership, as its writer would have written it. A record that
/// does not fit, or is written before the thread runs, first writes out what
/// is queued and then itself, as terminal output was written before.
const TX_BYTES: usize = 8192;
/// Each queued record is stored after its length in these many bytes.
const TX_HEADER: usize = 2;

/// Whole terminal records, each stored as its length (little endian) and its bytes.
struct TxRing {
    bytes: [u8; TX_BYTES],
    head: usize,
    len: usize,
}

impl TxRing {
    const fn new() -> Self {
        Self { bytes: [0; TX_BYTES], head: 0, len: 0 }
    }

    fn put(&mut self, byte: u8) {
        if let Some(slot) = self.bytes.get_mut((self.head + self.len) % TX_BYTES) {
            *slot = byte;
        }
        self.len += 1;
    }

    fn take(&mut self) -> u8 {
        let byte = self.bytes.get(self.head).copied().unwrap_or(0);
        self.head = (self.head + 1) % TX_BYTES;
        self.len -= 1;
        byte
    }

    /// Queue record `bytes` whole, or not at all when it does not fit.
    fn push(&mut self, bytes: &[u8]) -> bool {
        if bytes.len() > CAPACITY || TX_BYTES - self.len < TX_HEADER + bytes.len() {
            return false;
        }
        for byte in (bytes.len() as u16).to_le_bytes() {
            self.put(byte);
        }
        for &byte in bytes {
            self.put(byte);
        }
        true
    }

    /// Take the oldest record into `out`; its length, or 0 with the ring empty.
    fn pop(&mut self, out: &mut [u8; CAPACITY]) -> usize {
        if self.len < TX_HEADER {
            return 0;
        }
        let n = u16::from_le_bytes([self.take(), self.take()]) as usize;
        for index in 0..n {
            let byte = self.take();
            if let Some(slot) = out.get_mut(index) {
                *slot = byte;
            }
        }
        n.min(CAPACITY)
    }
}

/// Held only around copies, with interrupts masked; the UART is written after
/// it is released, and it is the innermost lock its holders take.
static TX: spin::Mutex<TxRing> = spin::Mutex::new(TxRing::new());
/// The transmit thread, once it runs; 0 before.
static TX_TID: AtomicU64 = AtomicU64::new(0);
/// The transmit thread is blocked with the ring empty (IDLE) or has been woken
/// to send (RUNNING). Writers move IDLE to RUNNING and wake it; only the thread
/// moves RUNNING to IDLE.
static TX_STATE: AtomicU8 = AtomicU8::new(TX_IDLE);
const TX_IDLE: u8 = 0;
const TX_RUNNING: u8 = 1;
/// Terminal output is stopped (IXON): queued output waits.
static TX_STOPPED: AtomicBool = AtomicBool::new(false);

fn with_ring<R>(f: impl FnOnce(&mut TxRing) -> R) -> R {
    let saved = Uart::mask_interrupts();
    let r = f(&mut TX.lock());
    Uart::restore_interrupts(saved);
    r
}

/// Queue terminal record `bytes` for the transmit thread, which holds it while
/// the terminal's output is stopped. False, leaving the ring unchanged, before
/// the thread runs or when the record does not fit: the caller writes it itself.
fn queue_terminal(bytes: &[u8]) -> bool {
    let tid = TX_TID.load(Ordering::Acquire);
    if tid == 0 || !with_ring(|ring| ring.push(bytes)) {
        return false;
    }
    if TX_STATE.swap(TX_RUNNING, Ordering::AcqRel) == TX_IDLE {
        crate::task::scheduler::wake_thread_any_context(tid);
    }
    true
}

/// Give the UART the oldest queued terminal record, popped and written under
/// one ownership, so records keep their order and each goes out whole. Returns
/// the ring bytes it took, or 0 when the ring was empty.
fn send_queued() -> usize {
    let mut ownership = Ownership::acquire();
    let mut record = [0u8; CAPACITY];
    let n = with_ring(|ring| ring.pop(&mut record));
    if n == 0 {
        return 0;
    }
    if let Some(sent) = record.get(..n) {
        uart_owner::write::<Uart, false>(Writer::Tty, sent, Kind::Plain, &mut ownership);
    }
    TX_HEADER + n
}

/// Write out the terminal records queued so far, one at a time, unless the
/// terminal's output is stopped. Records queued meanwhile are the transmit
/// thread's.
fn write_queued() {
    if TX_STOPPED.load(Ordering::Acquire) {
        return;
    }
    let mut left = with_ring(|ring| ring.len);
    while left != 0 {
        let n = send_queued();
        if n == 0 {
            break;
        }
        left = left.saturating_sub(n);
    }
}

/// Block the transmit thread until a writer hands it output (RUNNING). It
/// blocks for I/O, the one blocked state a wake from interrupt context (a
/// terminal echo) applies to. The state is checked after the block is
/// published, so a wake sent before it unblocks the thread at once.
fn wait_for_output(tid: u64) {
    while TX_STATE.load(Ordering::Acquire) != TX_RUNNING {
        crate::task::scheduler::with_scheduler(|sched| {
            sched.block_current_for_io_with_timeout(None);
            if TX_STATE.load(Ordering::Acquire) == TX_RUNNING {
                sched.unblock(tid);
            }
        });
        crate::task::scheduler::yield_current();
        crate::arch_halt_while_blocked();
    }
}

/// The terminal transmit thread: give the UART queued terminal records one at a
/// time, then block until a writer queues more.
fn terminal_transmitter() {
    let Some(tid) = crate::task::scheduler::current_thread_id() else { return };
    TX_TID.store(tid, Ordering::Release);
    loop {
        wait_for_output(tid);
        while !TX_STOPPED.load(Ordering::Acquire) && send_queued() != 0 {}
        TX_STATE.store(TX_IDLE, Ordering::Release);
        // A writer that queued after the last pop saw RUNNING and did not wake us.
        if !TX_STOPPED.load(Ordering::Acquire) && with_ring(|ring| ring.len) != 0 {
            TX_STATE.store(TX_RUNNING, Ordering::Release);
        }
    }
}

/// Start the terminal transmit thread; once it runs, terminal output is queued
/// for it rather than written by its writers. Called once, when kernel threads
/// can run.
pub fn start_transmitter() {
    if let Err(error) = crate::task::kthread::kthread_run(terminal_transmitter, "ktty_tx") {
        crate::serial_println!("[serial] no terminal transmit thread: {:?}", error);
    }
}

/// Wait until the terminal output queued so far has gone to the UART (TCSETSW,
/// TCSETSF): the caller writes it out itself.
pub fn drain_output() {
    write_queued();
}

/// Discard queued terminal output (TCFLSH, TCSETSF).
pub fn flush_output() {
    with_ring(|ring| {
        ring.head = 0;
        ring.len = 0;
    });
}

/// Stop or resume terminal output (IXON). Resuming wakes the transmit thread.
pub fn set_output_stopped(stopped: bool) {
    if TX_STOPPED.swap(stopped, Ordering::AcqRel) && !stopped {
        let tid = TX_TID.load(Ordering::Acquire);
        if tid != 0 && TX_STATE.swap(TX_RUNNING, Ordering::AcqRel) == TX_IDLE {
            crate::task::scheduler::wake_thread_any_context(tid);
        }
    }
}

/// A record assembled on the stack. A newline completes the record and writes
/// it; dropping the buffer writes any unterminated remainder.
/// Raw records do not drain deferred log notices.
pub type Line = Buffer<false>;
/// A [`Line`] whose bytes also go to the kernel log capture rings, draining
/// deferred notices first when written from thread context.
pub type TeeLine = Buffer<true>;

/// Write a caller-defined record, with the terminal queue available only to
/// this entry point. Raw diagnostic buffers cannot link the scheduler wake.
pub fn write_bytes(writer: Writer, bytes: &[u8]) {
    if writer == Writer::Tty {
        Buffer::<false, true>::for_writer(writer).bytes(bytes);
    } else {
        Line::for_writer(writer).bytes(bytes);
    }
}

pub struct Buffer<const TEE: bool, const TERMINAL: bool = false> {
    bytes: [MaybeUninit<u8>; CAPACITY],
    len: usize,
    writer: Writer,
    level: log::Level,
    /// Whether the bytes count toward the current thread (`output_accounting`).
    attributed: bool,
}

impl<const TEE: bool, const TERMINAL: bool> Buffer<TEE, TERMINAL> {
    /// A record of kernel output.
    pub const fn new() -> Self {
        Self::for_writer(Writer::Kernel)
    }

    pub const fn for_writer(writer: Writer) -> Self {
        Self {
            bytes: [MaybeUninit::uninit(); CAPACITY],
            len: 0,
            writer,
            level: log::Level::Info,
            attributed: true,
        }
    }

    /// A record of kernel output that names no thread (`emergency_print`): its bytes
    /// count as unattributed.
    pub const fn unattributed() -> Self {
        Self {
            bytes: [MaybeUninit::uninit(); CAPACITY],
            len: 0,
            writer: Writer::Kernel,
            level: log::Level::Info,
            attributed: false,
        }
    }

    /// Carry a structured kernel record's severity through UART chunking.
    pub fn with_level(level: log::Level) -> Self {
        let mut line = Self::new();
        line.level = level;
        line
    }

    pub fn char(&mut self, byte: u8) {
        if self.len >= CAPACITY {
            self.flush();
        }
        // `get_mut` rather than indexing: the timer-interrupt lockup report
        // assembles records here and must not reach a panic path.
        if let Some(slot) = self.bytes.get_mut(self.len) {
            slot.write(byte);
            self.len += 1;
        }
        if byte == b'\n' {
            self.flush();
        }
    }

    pub fn bytes(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.char(byte);
        }
    }

    pub fn text(&mut self, text: &str) {
        self.bytes(text.as_bytes());
    }

    pub fn newline(&mut self) {
        self.bytes(b"\r\n");
    }

    pub fn dec(&mut self, mut value: u64) {
        let mut digits = [0; 20];
        let mut start = digits.len();
        loop {
            start -= 1;
            digits[start] = b'0' + (value % 10) as u8;
            value /= 10;
            if value == 0 {
                break;
            }
        }
        self.bytes(&digits[start..]);
    }

    pub fn hex(&mut self, value: u64) {
        self.text("0x");
        let mut started = false;
        for shift in (0..16).rev() {
            let digit = ((value >> (shift * 4)) & 15) as usize;
            if digit != 0 || started || shift == 0 {
                self.char(b"0123456789abcdef"[digit]);
                started = true;
            }
        }
    }

    pub fn hex32(&mut self, value: u32) {
        self.text("0x");
        for shift in (0..8).rev() {
            self.char(b"0123456789abcdef"[((value >> (shift * 4)) & 15) as usize]);
        }
    }

    pub fn hex16(&mut self, value: u16) {
        self.text("0x");
        for shift in (0..4).rev() {
            self.char(b"0123456789abcdef"[((value >> (shift * 4)) & 15) as usize]);
        }
    }

    fn flush(&mut self) {
        // `char` initializes every byte below `len`.
        let record =
            unsafe { core::slice::from_raw_parts(self.bytes.as_ptr().cast::<u8>(), self.len) };
        emit::<TEE, TERMINAL>(self.writer, record, self.level, self.attributed);
        self.len = 0;
    }
}

impl<const TEE: bool, const TERMINAL: bool> core::fmt::Write for Buffer<TEE, TERMINAL> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        self.text(s);
        Ok(())
    }
}

impl<const TEE: bool, const TERMINAL: bool> Drop for Buffer<TEE, TERMINAL> {
    fn drop(&mut self) {
        self.flush();
    }
}
