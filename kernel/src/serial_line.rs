//! Aarch64 UART ownership (issue 847).
//!
//! Every aarch64 UART writer goes through this module. A record (a line, or a
//! caller-defined unterminated unit such as a TTY character or a breadcrumb)
//! is assembled on the caller's stack, then written to the UART in one piece
//! while this CPU owns the UART (`uart_owner`, which has the ownership rules).
//!
//! Emission is synchronous. Nothing is queued: a record reaches the UART
//! before the call that completes it returns. A record longer than the
//! assembly buffer is written in buffer-sized pieces, each one owned. The one
//! writer that may discard its record is [`try_write`], for breadcrumbs on
//! paths that must not wait.
//!
//! Lines posted on paths that may not log (`log_notice`) are written here from
//! thread context only: by the next logged record a thread writes, and by the idle
//! loop ([`write_notices_from_thread`]). Each is a whole line of its own.
use crate::serial_writer::Writer;
use crate::uart_owner::{self, Kind, Port};
use core::mem::MaybeUninit;

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

/// Write one record from `writer` to the UART while owning it, on a fresh
/// line if another writer's line is open.
fn emit<const TEE: bool>(writer: Writer, bytes: &[u8], level: log::Level) {
    if bytes.is_empty() {
        return;
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

/// [`write_notices`], from thread context only: never from an interrupt or
/// softirq, nor with interrupts masked, so interrupt paths and the timer's
/// lockup report never write the list.
pub fn write_notices_from_thread() {
    if crate::log_notice::pending()
        && crate::per_cpu::is_initialized()
        && crate::arch_interrupts_enabled()
        && !crate::per_cpu::in_interrupt()
    {
        write_notices();
    }
}

/// Write `bytes` as one record if the UART can be owned without waiting, for
/// breadcrumbs on paths that must not wait on another CPU's output. Returns
/// false, having written nothing, when another CPU owns or is waiting for it.
pub fn try_write(bytes: &[u8]) -> bool {
    match Ownership::try_acquire() {
        Some(mut ownership) => {
            uart_owner::write::<Uart, false>(Writer::Kernel, bytes, Kind::Plain, &mut ownership);
            true
        }
        None => false,
    }
}

/// A record assembled on the stack. A newline completes the record and writes
/// it; dropping the buffer writes any unterminated remainder.
/// Raw records do not drain deferred log notices.
pub type Line = Buffer<false>;
/// A [`Line`] whose bytes also go to the kernel log capture rings, draining
/// deferred notices first when written from thread context.
pub type TeeLine = Buffer<true>;

pub struct Buffer<const TEE: bool> {
    bytes: [MaybeUninit<u8>; CAPACITY],
    len: usize,
    writer: Writer,
    level: log::Level,
}

impl<const TEE: bool> Buffer<TEE> {
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
        emit::<TEE>(self.writer, record, self.level);
        self.len = 0;
    }
}

impl<const TEE: bool> core::fmt::Write for Buffer<TEE> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        self.text(s);
        Ok(())
    }
}

impl<const TEE: bool> Drop for Buffer<TEE> {
    fn drop(&mut self) {
        self.flush();
    }
}
