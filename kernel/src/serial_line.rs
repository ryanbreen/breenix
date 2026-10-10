//! Aarch64 UART ownership (issue 847).
//!
//! Every aarch64 UART writer goes through this module. A record (a line, or a
//! caller-defined unterminated unit such as a TTY character or a breadcrumb)
//! is assembled on the caller's stack, then written to the UART in one piece
//! while this CPU owns the UART. Records from different CPUs therefore never
//! interleave.
//!
//! Emission is synchronous. Nothing is queued: a record reaches the UART
//! before the call that completes it returns. A record longer than the
//! assembly buffer is written in buffer-sized pieces, each one owned. The one
//! writer that may discard its record is [`try_write`], for breadcrumbs on
//! paths that must not wait.
//!
//! Ownership is a ticket lock, so CPUs own the UART in the order they asked
//! for it and a waiter waits at most for the records already ahead of it, not
//! for as long as other CPUs keep writing. It is held only while bytes are
//! written, with IRQs and FIQs masked, so an owner waits on nothing but the
//! UART itself, and that wait is bounded too ([`READY_NS`]). It is reentrant
//! on the owning CPU: a fault or panic taken while this CPU owns the UART
//! writes its own output inline instead of waiting on itself.
//!
//! An owner that writes nothing for [`STALL_NS`] (it halted, faulted and never
//! returned, or its vCPU was descheduled) has its ticket skipped by a waiter.
//! Skipping revokes it: an owner checks its ticket before every byte, and one
//! that finds it skipped queues again behind the CPUs now waiting. At most the
//! byte already past that check can land inside another CPU's record. Fatal
//! output therefore depends on no other CPU and on no scheduler progress.
use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

/// Assembly buffer size. Records up to this length are written in one owned piece.
const CAPACITY: usize = 1024;

/// How long a waiter lets the owner go without writing a byte before skipping it.
const STALL_NS: u64 = 100_000_000;

/// How long one byte waits for the UART to accept it before being written
/// anyway, as Linux's 8250 console does.
const READY_NS: u64 = 10_000_000;

/// The next ticket to hand out.
static NEXT: AtomicU32 = AtomicU32::new(0);

/// The ticket that owns the UART. It advances when the owner releases, or
/// when a waiter skips an owner that stalled.
static SERVING: AtomicU32 = AtomicU32::new(0);

/// The owner's ticket (high half) and CPU tag (low half), written once it owns
/// the UART. Only its own CPU acts on it, to recognise reentry.
static HOLDER: AtomicU64 = AtomicU64::new(0);

/// Bytes written by any owner. A waiter that sees this advance knows the owner is alive.
static PROGRESS: AtomicU64 = AtomicU64::new(0);

/// Set when a byte timed out waiting for the UART; later bytes do not wait
/// until the UART is seen ready again.
static WEDGED: AtomicBool = AtomicBool::new(false);

/// Tag of the CPU writing into the log capture rings, or 0.
static CAPTURING: AtomicU32 = AtomicU32::new(0);

/// A nonzero value identifying this CPU, read from MPIDR so it is valid before
/// per-CPU state exists.
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

fn ticks(ns: u64) -> u64 {
    let frequency: u64;
    unsafe {
        core::arch::asm!("mrs {}, cntfrq_el0", out(reg) frequency, options(nomem, nostack, preserves_flags));
    }
    // A zero CNTFRQ (misconfigured firmware) still gets a finite wait.
    frequency.max(1_000_000) / (1_000_000_000 / ns)
}

fn mask_interrupts() -> u64 {
    let daif: u64;
    unsafe {
        core::arch::asm!("mrs {}, daif", out(reg) daif, options(nomem, nostack, preserves_flags));
        core::arch::asm!("msr daifset, #0x3", options(nomem, nostack, preserves_flags));
    }
    daif
}

fn restore_interrupts(daif: u64) {
    unsafe {
        core::arch::asm!("msr daif, {}", in(reg) daif, options(nomem, nostack, preserves_flags));
    }
}

/// Take a ticket and wait until it owns the UART.
fn take(tag: u32) -> u32 {
    let stall = ticks(STALL_NS);
    loop {
        let ticket = NEXT.fetch_add(1, Ordering::Relaxed);
        if wait(ticket, stall) {
            HOLDER.store(((ticket as u64) << 32) | tag as u64, Ordering::Relaxed);
            return ticket;
        }
        // This CPU was not running when its turn came, and a waiter skipped it.
    }
}

/// Wait for `ticket` to be served. False if it was skipped instead.
fn wait(ticket: u32, stall: u64) -> bool {
    let mut watched = (SERVING.load(Ordering::Acquire), PROGRESS.load(Ordering::Relaxed));
    let mut since = counter();
    loop {
        let serving = SERVING.load(Ordering::Acquire);
        if serving == ticket {
            return true;
        }
        if (ticket.wrapping_sub(serving) as i32) < 0 {
            return false;
        }
        let now = counter();
        let seen = (serving, PROGRESS.load(Ordering::Relaxed));
        if seen != watched {
            watched = seen;
            since = now;
        } else if now.wrapping_sub(since) >= stall {
            // The ticket being served has written nothing for STALL_NS. Skip it;
            // if another waiter got there first, this does nothing.
            let _ = SERVING.compare_exchange(
                serving,
                serving.wrapping_add(1),
                Ordering::AcqRel,
                Ordering::Relaxed,
            );
            since = now;
        }
        core::hint::spin_loop();
    }
}

/// Proof that this CPU owns the UART. Only `serial_line` creates one.
pub(crate) struct Ownership {
    /// DAIF on entry, restored when ownership ends.
    daif: u64,
    tag: u32,
    ticket: u32,
    /// This guard took its ticket and must release it. False when the CPU
    /// already owned the UART (a fault or panic inside an emission).
    release: bool,
}

impl Ownership {
    /// The ticket this CPU already owns, if any.
    fn reentered(tag: u32) -> Option<u32> {
        let holder = HOLDER.load(Ordering::Relaxed);
        let ticket = (holder >> 32) as u32;
        (holder as u32 == tag && SERVING.load(Ordering::Acquire) == ticket).then_some(ticket)
    }

    fn acquire() -> Self {
        let daif = mask_interrupts();
        let tag = cpu_tag();
        match Self::reentered(tag) {
            Some(ticket) => Self {
                daif,
                tag,
                ticket,
                release: false,
            },
            None => Self {
                daif,
                tag,
                ticket: take(tag),
                release: true,
            },
        }
    }

    /// Own the UART only if that needs no wait: it is free, or this CPU owns it.
    fn try_acquire() -> Option<Self> {
        let daif = mask_interrupts();
        let tag = cpu_tag();
        if let Some(ticket) = Self::reentered(tag) {
            return Some(Self {
                daif,
                tag,
                ticket,
                release: false,
            });
        }
        // With no ticket outstanding, NEXT equals SERVING and the next ticket owns at once.
        let serving = SERVING.load(Ordering::Acquire);
        if NEXT
            .compare_exchange(serving, serving.wrapping_add(1), Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
        {
            HOLDER.store(((serving as u64) << 32) | tag as u64, Ordering::Relaxed);
            return Some(Self {
                daif,
                tag,
                ticket: serving,
                release: true,
            });
        }
        restore_interrupts(daif);
        None
    }

    /// False once a waiter has skipped this ticket after this CPU stalled.
    fn held(&self) -> bool {
        SERVING.load(Ordering::Acquire) == self.ticket
    }

    /// Own the UART again after losing it, behind the CPUs already waiting.
    fn regain(&mut self) {
        self.ticket = take(self.tag);
        self.release = true;
    }
}

impl Drop for Ownership {
    fn drop(&mut self) {
        if self.release {
            // Fails only if a waiter skipped this ticket; SERVING has moved on.
            let _ = SERVING.compare_exchange(
                self.ticket,
                self.ticket.wrapping_add(1),
                Ordering::Release,
                Ordering::Relaxed,
            );
        }
        restore_interrupts(self.daif);
    }
}

/// Wait up to [`READY_NS`] for the UART to accept a byte.
fn wait_ready() {
    if crate::serial_aarch64::hardware_ready() {
        if WEDGED.load(Ordering::Relaxed) {
            WEDGED.store(false, Ordering::Relaxed);
        }
        return;
    }
    if WEDGED.load(Ordering::Relaxed) {
        return;
    }
    let limit = ticks(READY_NS);
    let start = counter();
    while !crate::serial_aarch64::hardware_ready() {
        if counter().wrapping_sub(start) >= limit {
            WEDGED.store(true, Ordering::Relaxed);
            return;
        }
        core::hint::spin_loop();
    }
}

/// Feed the log capture rings. They are single-producer; the owner is
/// normally the only writer, but a CPU whose ticket was skipped may still be
/// inside them, so they have their own exclusion. A byte that finds another
/// writer there (or a fault taken inside them on this CPU) goes to the UART only.
fn capture(byte: u8, tag: u32) {
    if CAPTURING
        .compare_exchange(0, tag, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        return;
    }
    crate::graphics::log_capture::capture_byte(byte);
    crate::log_buffer::capture_byte(byte);
    CAPTURING.store(0, Ordering::Release);
}

fn write<const TEE: bool>(bytes: &[u8], ownership: &mut Ownership) {
    for &byte in bytes {
        if !ownership.held() {
            ownership.regain();
        }
        wait_ready();
        crate::serial_aarch64::hardware_write(byte, ownership);
        PROGRESS.fetch_add(1, Ordering::Relaxed);
        if TEE {
            capture(byte, ownership.tag);
        }
    }
}

/// Write one record to the UART while owning it.
fn emit<const TEE: bool>(bytes: &[u8]) {
    if bytes.is_empty() {
        return;
    }
    let tmp_t0 = crate::signal::types::tmpring::now();
    let mut ownership = Ownership::acquire();
    write::<TEE>(bytes, &mut ownership);
    {
        let t1 = crate::signal::types::tmpring::now();
        if t1 - tmp_t0 > 200 { crate::signal::types::tmpring::rec(crate::signal::types::tmpring::K_SERIAL, t1, ((t1 - tmp_t0) << 16) | (bytes.len() as u64 & 0xffff)); }
    }
}

/// Write `bytes` as one record if the UART can be owned without waiting, for
/// breadcrumbs on paths that must not wait on another CPU's output. Returns
/// false, having written nothing, when another CPU owns or is waiting for it.
pub fn try_write(bytes: &[u8]) -> bool {
    match Ownership::try_acquire() {
        Some(mut ownership) => {
            write::<false>(bytes, &mut ownership);
            true
        }
        None => false,
    }
}

/// A record assembled on the stack. A newline completes the record and writes
/// it; dropping the buffer writes any unterminated remainder.
pub type Line = Buffer<false>;
/// A [`Line`] whose bytes also go to the kernel log capture rings.
pub type TeeLine = Buffer<true>;

pub struct Buffer<const TEE: bool> {
    bytes: [MaybeUninit<u8>; CAPACITY],
    len: usize,
}

impl<const TEE: bool> Buffer<TEE> {
    pub const fn new() -> Self {
        Self {
            bytes: [MaybeUninit::uninit(); CAPACITY],
            len: 0,
        }
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
        emit::<TEE>(record);
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
