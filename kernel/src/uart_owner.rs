//! UART ownership and record writing (issue 847), for any port.
//!
//! `serial_line` runs this over the aarch64 UART. It is generic over the port
//! ([`Port`]) so that the host test `tests/uart_owner.rs` can run it with
//! threads standing in for CPUs. Core only.
//!
//! A record is written to the UART in one piece while this CPU owns the UART,
//! so other CPUs' bytes do not land inside it unless its owner stalls (below).
//! A record that begins while another writer's line is still open starts a
//! fresh line first (`serial_writer`).
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
//! that finds it skipped queues again behind the CPUs now waiting, and starts
//! a fresh line if another writer's line was left open meanwhile; its record
//! is then split around the other CPUs' output. Fatal output therefore
//! depends on no other CPU and on no scheduler progress.
//!
//! An owner that stalls between that check and its byte writes the byte late,
//! into whatever another CPU is writing by then. Every byte is therefore
//! bracketed by two counters ([`State::begun`], [`State::ended`]), and a record
//! that must reach the UART whole sees from them whether another CPU wrote
//! while it was being written. Two kinds of record must: one carrying a
//! boot-path stage marker (`stage_lines`), and a [`Kind::Line`]. Such a record
//! that loses its ownership part way, or that another CPU wrote into, is
//! written again from its first byte, on a fresh line.

use core::marker::PhantomData;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use super::stage_lines;

/// How long a waiter lets the owner go without writing a byte before skipping it.
pub const STALL_NS: u64 = 100_000_000;

/// How long one byte waits for the UART to accept it before being written
/// anyway, as Linux's 8250 console does.
pub const READY_NS: u64 = 10_000_000;

/// The UART and the CPU a writer runs on.
pub trait Port: Sized + 'static {
    /// Who a record belongs to, for starting fresh lines (`serial_writer`).
    type Writer: Copy;
    /// This port's ownership.
    fn state() -> &'static State;
    /// A nonzero value identifying the calling CPU.
    fn cpu_tag() -> u32;
    /// A free-running counter.
    fn counter() -> u64;
    /// Counter ticks in `ns` nanoseconds.
    fn ticks(ns: u64) -> u64;
    /// Mask interrupts, returning what to restore.
    fn mask_interrupts() -> u64;
    fn restore_interrupts(saved: u64);
    /// The UART can accept a byte.
    fn ready() -> bool;
    /// Send one byte. Only an owner calls this.
    fn write(byte: u8, owner: &Ownership<Self>);
    /// Feed a byte to the log capture rings.
    fn capture(byte: u8);
    /// What `writer` writes ahead of `unit` (`serial_writer::lead`).
    fn lead(writer: Self::Writer, unit: &[u8]) -> &'static [u8];
    /// Some writer's line is open (`serial_writer::line_open`).
    fn line_open() -> bool;
    /// `writer` wrote `byte` last (`serial_writer::wrote`).
    fn wrote(writer: Self::Writer, byte: u8);
    /// The boot-path stage markers (`stage_lines`).
    fn markers() -> &'static [&'static str];
}

/// A port's ownership.
pub struct State {
    /// The next ticket to hand out.
    next: AtomicU32,
    /// The ticket that owns the UART. It advances when the owner releases, or
    /// when a waiter skips an owner that stalled.
    serving: AtomicU32,
    /// The owner's ticket (high half) and CPU tag (low half), written once it
    /// owns the UART. Only its own CPU acts on it, to recognise reentry.
    holder: AtomicU64,
    /// Bytes begun: counted before the writer checks it still owns the UART.
    begun: AtomicU64,
    /// Bytes ended: counted after the byte is written, or found not to be the
    /// writer's to write. A waiter that sees this advance knows the owner is alive.
    ended: AtomicU64,
    /// Set when a byte timed out waiting for the UART; later bytes do not wait
    /// until the UART is seen ready again.
    wedged: AtomicBool,
    /// Tag of the CPU writing into the log capture rings, or 0.
    capturing: AtomicU32,
}

impl State {
    pub const fn new() -> Self {
        Self {
            next: AtomicU32::new(0),
            serving: AtomicU32::new(0),
            holder: AtomicU64::new(0),
            begun: AtomicU64::new(0),
            ended: AtomicU64::new(0),
            wedged: AtomicBool::new(false),
            capturing: AtomicU32::new(0),
        }
    }
}

impl Default for State {
    fn default() -> Self {
        Self::new()
    }
}

/// Take a ticket and wait until it owns the UART.
fn take<P: Port>(tag: u32) -> u32 {
    let state = P::state();
    let stall = P::ticks(STALL_NS);
    loop {
        let ticket = state.next.fetch_add(1, Ordering::Relaxed);
        if wait::<P>(ticket, stall) {
            state
                .holder
                .store(((ticket as u64) << 32) | tag as u64, Ordering::Relaxed);
            return ticket;
        }
        // This CPU was not running when its turn came, and a waiter skipped it.
    }
}

/// Wait for `ticket` to be served. False if it was skipped instead.
fn wait<P: Port>(ticket: u32, stall: u64) -> bool {
    let state = P::state();
    let mut watched = (
        state.serving.load(Ordering::SeqCst),
        state.ended.load(Ordering::Relaxed),
    );
    let mut since = P::counter();
    loop {
        let serving = state.serving.load(Ordering::SeqCst);
        if serving == ticket {
            return true;
        }
        if (ticket.wrapping_sub(serving) as i32) < 0 {
            return false;
        }
        let now = P::counter();
        let seen = (serving, state.ended.load(Ordering::Relaxed));
        if seen != watched {
            watched = seen;
            since = now;
        } else if now.wrapping_sub(since) >= stall {
            // The ticket being served has written nothing for STALL_NS. Skip it;
            // if another waiter got there first, this does nothing.
            let _ = state.serving.compare_exchange(
                serving,
                serving.wrapping_add(1),
                Ordering::SeqCst,
                Ordering::Relaxed,
            );
            since = now;
        }
        core::hint::spin_loop();
    }
}

/// Proof that this CPU owns the UART.
pub struct Ownership<P: Port> {
    /// Interrupt state on entry, restored when ownership ends.
    saved: u64,
    tag: u32,
    ticket: u32,
    /// This guard took its ticket and must release it. False when the CPU
    /// already owned the UART (a fault or panic inside an emission).
    release: bool,
    /// Bytes this guard has begun.
    puts: u64,
    port: PhantomData<P>,
}

impl<P: Port> Ownership<P> {
    /// The ticket this CPU already owns, if any.
    fn reentered(tag: u32) -> Option<u32> {
        let state = P::state();
        let holder = state.holder.load(Ordering::Relaxed);
        let ticket = (holder >> 32) as u32;
        (holder as u32 == tag && state.serving.load(Ordering::SeqCst) == ticket).then_some(ticket)
    }

    fn new(saved: u64, tag: u32, ticket: u32, release: bool) -> Self {
        Self {
            saved,
            tag,
            ticket,
            release,
            puts: 0,
            port: PhantomData,
        }
    }

    /// Own the UART, waiting for the CPUs ahead.
    pub fn acquire() -> Self {
        let saved = P::mask_interrupts();
        let tag = P::cpu_tag();
        match Self::reentered(tag) {
            Some(ticket) => Self::new(saved, tag, ticket, false),
            None => Self::new(saved, tag, take::<P>(tag), true),
        }
    }

    /// Own the UART only if that needs no wait: it is free, or this CPU owns it.
    pub fn try_acquire() -> Option<Self> {
        let state = P::state();
        let saved = P::mask_interrupts();
        let tag = P::cpu_tag();
        if let Some(ticket) = Self::reentered(tag) {
            return Some(Self::new(saved, tag, ticket, false));
        }
        // With no ticket outstanding, NEXT equals SERVING and the next ticket owns at once.
        let serving = state.serving.load(Ordering::SeqCst);
        if state
            .next
            .compare_exchange(serving, serving.wrapping_add(1), Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
        {
            state
                .holder
                .store(((serving as u64) << 32) | tag as u64, Ordering::Relaxed);
            return Some(Self::new(saved, tag, serving, true));
        }
        P::restore_interrupts(saved);
        None
    }

    /// False once a waiter has skipped this ticket after this CPU stalled.
    fn held(&self) -> bool {
        P::state().serving.load(Ordering::SeqCst) == self.ticket
    }

    /// Own the UART again after losing it, behind the CPUs already waiting.
    fn regain(&mut self) {
        self.ticket = take::<P>(self.tag);
        self.release = true;
    }
}

impl<P: Port> Drop for Ownership<P> {
    fn drop(&mut self) {
        if self.release {
            // Fails only if a waiter skipped this ticket; SERVING has moved on.
            let _ = P::state().serving.compare_exchange(
                self.ticket,
                self.ticket.wrapping_add(1),
                Ordering::SeqCst,
                Ordering::Relaxed,
            );
        }
        P::restore_interrupts(self.saved);
    }
}

/// Wait up to [`READY_NS`] for the UART to accept a byte.
fn wait_ready<P: Port>() {
    let state = P::state();
    if P::ready() {
        if state.wedged.load(Ordering::Relaxed) {
            state.wedged.store(false, Ordering::Relaxed);
        }
        return;
    }
    if state.wedged.load(Ordering::Relaxed) {
        return;
    }
    let limit = P::ticks(READY_NS);
    let start = P::counter();
    while !P::ready() {
        if P::counter().wrapping_sub(start) >= limit {
            state.wedged.store(true, Ordering::Relaxed);
            return;
        }
        core::hint::spin_loop();
    }
}

/// Feed the log capture rings. They are single-producer; the owner is
/// normally the only writer, but a CPU whose ticket was skipped may still be
/// inside them, so they have their own exclusion. A byte that finds another
/// writer there (or a fault taken inside them on this CPU) goes to the UART only.
fn capturing<P: Port>(tag: u32, write: impl FnOnce()) -> bool {
    let state = P::state();
    if state
        .capturing
        .compare_exchange(0, tag, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        return false;
    }
    write();
    state.capturing.store(0, Ordering::Release);
    true
}

fn capture<P: Port>(byte: u8, tag: u32) {
    capturing::<P>(tag, || P::capture(byte));
}

/// Capture a record without writing the UART. Own the UART to keep ordinary
/// loggers out, and share their capture exclusion even if this ticket is
/// skipped or a fault reenters. A busy capture is dropped rather than waited on.
pub fn capture_only<P: Port>(args: core::fmt::Arguments) -> bool {
    struct Capture<P>(PhantomData<P>);
    impl<P: Port> core::fmt::Write for Capture<P> {
        fn write_str(&mut self, text: &str) -> core::fmt::Result {
            text.bytes().for_each(P::capture);
            Ok(())
        }
    }
    let ownership = Ownership::<P>::acquire();
    capturing::<P>(ownership.tag, || {
        let _ = core::fmt::Write::write_fmt(&mut Capture::<P>(PhantomData), args);
    })
}

/// Write `byte` if this CPU still owns the UART. False, having written
/// nothing, once a waiter has skipped its ticket.
///
/// The byte is counted begun before the ownership check and ended after the
/// write, both SeqCst like the check and the skip. So a CPU that checks
/// before a waiter skips it, and writes after, is still counted begun when the
/// new owner opens its [`Window`], and its byte ends inside that window.
fn put<P: Port, const TEE: bool>(writer: P::Writer, byte: u8, ownership: &mut Ownership<P>) -> bool {
    let state = P::state();
    ownership.puts += 1;
    state.begun.fetch_add(1, Ordering::SeqCst);
    let held = ownership.held();
    if held {
        wait_ready::<P>();
        P::write(byte, ownership);
        P::wrote(writer, byte);
        if TEE {
            capture::<P>(byte, ownership.tag);
        }
    }
    state.ended.fetch_add(1, Ordering::SeqCst);
    held
}

/// Write `bytes` until one is refused, then report whether all went.
fn put_all<P: Port, const TEE: bool>(
    writer: P::Writer,
    bytes: &[u8],
    ownership: &mut Ownership<P>,
) -> bool {
    bytes
        .iter()
        .all(|&byte| put::<P, TEE>(writer, byte, ownership))
}

/// The byte counts when an owner began writing a record.
struct Window {
    begun: u64,
    ended: u64,
    puts: u64,
}

/// Whether another CPU wrote to the UART while a record was being written.
enum Crossed {
    No,
    Yes,
    /// Not so far, but a byte another CPU had begun before the record started
    /// has not ended.
    Pending,
}

impl Window {
    fn open<P: Port>(ownership: &Ownership<P>) -> Self {
        let state = P::state();
        Self {
            begun: state.begun.load(Ordering::SeqCst),
            ended: state.ended.load(Ordering::SeqCst),
            puts: ownership.puts,
        }
    }

    fn crossed<P: Port>(&self, ownership: &Ownership<P>) -> Crossed {
        let state = P::state();
        let mine = ownership.puts.wrapping_sub(self.puts);
        let begun = state.begun.load(Ordering::SeqCst).wrapping_sub(self.begun);
        let ended = state.ended.load(Ordering::SeqCst).wrapping_sub(self.ended);
        if begun != mine || ended != mine {
            Crossed::Yes
        } else if self.begun != self.ended {
            Crossed::Pending
        } else {
            Crossed::No
        }
    }

    /// Wait up to [`STALL_NS`] for a byte begun before the record to end. True
    /// if it did: it may have reached the UART inside the record. A byte that
    /// stays begun that long is the stall a waiter skips an owner for. A
    /// reentrant guard does not wait, as the byte is likely its own CPU's,
    /// interrupted by the fault it is reporting.
    fn settles_crossed<P: Port>(&self, ownership: &Ownership<P>) -> bool {
        if !ownership.release {
            return false;
        }
        let limit = P::ticks(STALL_NS);
        let start = P::counter();
        while P::counter().wrapping_sub(start) < limit {
            if !matches!(self.crossed(ownership), Crossed::Pending) {
                return true;
            }
            core::hint::spin_loop();
        }
        false
    }
}

/// What a record must survive.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Split around other CPUs' output if this CPU stalls, unless it carries a
    /// boot-path stage marker: then it is written whole.
    Plain,
    /// A whole line of its own, always: written whole, starting a fresh line
    /// whoever's line is open.
    Line,
}

/// Write one record from `writer` while owning the UART.
pub fn write<P: Port, const TEE: bool>(
    writer: P::Writer,
    bytes: &[u8],
    kind: Kind,
    ownership: &mut Ownership<P>,
) {
    if bytes.is_empty() {
        return;
    }
    // Decided the first time it matters: when the record loses bytes.
    let mut whole = (kind == Kind::Line).then_some(true);
    let is_whole = |whole: &mut Option<bool>| {
        *whole.get_or_insert_with(|| stage_lines::contains(P::markers(), bytes))
    };
    let mut fresh = kind == Kind::Line;
    'attempt: loop {
        let window = Window::open(ownership);
        let lead: &[u8] = if !fresh {
            P::lead(writer, bytes)
        } else if P::line_open() {
            b"\r\n"
        } else {
            b""
        };
        if !put_all::<P, TEE>(writer, lead, ownership) {
            ownership.regain();
            // A late CR from the lost lead can leave our own line open.
            // Rebuild the line boundary instead of treating it as payload
            // continuation and omitting the missing LF on the next attempt.
            fresh = true;
            continue 'attempt;
        }
        let mut index = 0;
        while index < bytes.len() {
            if put::<P, TEE>(writer, bytes[index], ownership) {
                index += 1;
                continue;
            }
            // Other CPUs wrote while this one waited; one may have left its line open.
            ownership.regain();
            if index == 0 {
                continue 'attempt;
            }
            if is_whole(&mut whole) {
                fresh = true;
                continue 'attempt;
            }
            // The rest goes on after the other CPUs' output.
            while !put_all::<P, TEE>(writer, P::lead(writer, &bytes[index..]), ownership) {
                ownership.regain();
            }
        }
        let rewrite = match window.crossed(ownership) {
            Crossed::No => false,
            Crossed::Yes => is_whole(&mut whole),
            Crossed::Pending => is_whole(&mut whole) && window.settles_crossed(ownership),
        };
        if !rewrite {
            return;
        }
        fresh = true;
    }
}
