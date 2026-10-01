//! Aarch64 UART ownership (issue 847).
//!
//! Every aarch64 UART writer goes through this module. A record (a line, or a
//! caller-defined unterminated unit such as a TTY character or a breadcrumb)
//! is assembled on the caller's stack, then written to the UART in one piece
//! while this CPU owns the UART. Records from different CPUs therefore never
//! interleave.
//!
//! Emission is synchronous. Nothing is queued and nothing is dropped: a record
//! reaches the UART before the call that completes it returns, so output
//! appears in the order records take ownership. A record longer than the
//! assembly buffer is written in buffer-sized pieces, each one owned.
//!
//! Ownership is held only while bytes are written, with IRQs and FIQs masked,
//! so an owner waits on nothing but the UART itself. It is reentrant on the
//! owning CPU: a fault or panic taken while this CPU owns the UART writes its
//! own output inline instead of waiting on itself. A CPU that finds another
//! CPU owning the UART waits for as long as that owner keeps writing bytes. If
//! the owner writes nothing for [`STALL_NS`] (it halted, or faulted and never
//! returned to release), the waiter takes ownership. Fatal output therefore
//! depends on no other CPU and on no scheduler progress.
use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicU64, Ordering};

/// Assembly buffer size. Records up to this length are written in one owned piece.
const CAPACITY: usize = 1024;

/// How long a waiter lets the owner go without writing a byte before taking over.
const STALL_NS: u64 = 100_000_000;

/// The owning CPU's tag ([`cpu_tag`]), or 0 when the UART is free.
static OWNER: AtomicU64 = AtomicU64::new(0);

/// Bytes written by any owner. A waiter that sees this advance knows the owner is alive.
static PROGRESS: AtomicU64 = AtomicU64::new(0);

/// A nonzero value identifying this CPU, read from MPIDR so it is valid before
/// per-CPU state exists.
#[inline(always)]
fn cpu_tag() -> u64 {
    let mpidr: u64;
    unsafe {
        core::arch::asm!("mrs {}, mpidr_el1", out(reg) mpidr, options(nomem, nostack, preserves_flags));
    }
    (mpidr & 0xFF_00FF_FFFF) + 1
}

#[inline(always)]
fn counter() -> u64 {
    let ticks: u64;
    unsafe {
        core::arch::asm!("isb", "mrs {}, cntvct_el0", out(reg) ticks, options(nomem, nostack, preserves_flags));
    }
    ticks
}

fn stall_ticks() -> u64 {
    let frequency: u64;
    unsafe {
        core::arch::asm!("mrs {}, cntfrq_el0", out(reg) frequency, options(nomem, nostack, preserves_flags));
    }
    // A zero CNTFRQ (misconfigured firmware) still gets a finite wait.
    frequency.max(1_000_000) / (1_000_000_000 / STALL_NS)
}

/// Proof that this CPU owns the UART. Only `serial_line` creates one.
pub(crate) struct Ownership {
    /// DAIF on entry, restored when ownership ends.
    daif: u64,
    /// This guard took ownership and must release it. False when the CPU
    /// already owned the UART (a fault or panic inside an emission).
    release: bool,
    /// Ownership was taken from a stalled owner.
    taken_over: bool,
}

impl Ownership {
    fn acquire() -> Self {
        let daif: u64;
        unsafe {
            core::arch::asm!("mrs {}, daif", out(reg) daif, options(nomem, nostack, preserves_flags));
            core::arch::asm!(
                "msr daifset, #0x3",
                options(nomem, nostack, preserves_flags)
            );
        }
        let tag = cpu_tag();
        let mut owner = OWNER.load(Ordering::Relaxed);
        if owner == tag {
            return Self {
                daif,
                release: false,
                taken_over: false,
            };
        }

        let stall = stall_ticks();
        let mut watched = (owner, PROGRESS.load(Ordering::Relaxed));
        let mut since = counter();
        loop {
            if owner == 0 {
                match OWNER.compare_exchange_weak(0, tag, Ordering::Acquire, Ordering::Relaxed) {
                    Ok(_) => {
                        return Self {
                            daif,
                            release: true,
                            taken_over: false,
                        }
                    }
                    Err(current) => owner = current,
                }
                continue;
            }
            let now = counter();
            let seen = (owner, PROGRESS.load(Ordering::Relaxed));
            if seen != watched {
                watched = seen;
                since = now;
            } else if now.wrapping_sub(since) >= stall
                && OWNER
                    .compare_exchange(owner, tag, Ordering::Acquire, Ordering::Relaxed)
                    .is_ok()
            {
                return Self {
                    daif,
                    release: true,
                    taken_over: true,
                };
            }
            core::hint::spin_loop();
            owner = OWNER.load(Ordering::Relaxed);
        }
    }
}

impl Drop for Ownership {
    fn drop(&mut self) {
        if self.release {
            // Fails only if a waiter took over after this owner stalled; the
            // UART is theirs now and stays theirs.
            let _ = OWNER.compare_exchange(cpu_tag(), 0, Ordering::Release, Ordering::Relaxed);
        }
        unsafe {
            core::arch::asm!("msr daif, {}", in(reg) self.daif, options(nomem, nostack, preserves_flags));
        }
    }
}

/// Write one record to the UART while owning it.
fn emit<const TEE: bool>(bytes: &[u8]) {
    if bytes.is_empty() {
        return;
    }
    let ownership = Ownership::acquire();
    // The capture rings are single-producer. After a takeover the stalled
    // owner may still be partway through its own capture, so a record that
    // took over goes to the UART only.
    let tee = TEE && !ownership.taken_over;
    for &byte in bytes {
        crate::serial_aarch64::hardware_byte(byte, &ownership);
        PROGRESS.fetch_add(1, Ordering::Relaxed);
        if tee {
            crate::graphics::log_capture::capture_byte(byte);
            crate::log_buffer::capture_byte(byte);
        }
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
