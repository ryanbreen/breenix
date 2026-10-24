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
//! Non-fatal atomic records use a bounded deferred ring. They release UART
//! ownership between sixteen-byte pieces, with interrupts restored between them.
//! Fatal output therefore depends on no other CPU and on no scheduler progress.

#[path = "serial_defer.rs"]
pub mod defer;

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
    type Writer: Copy + Send;
    /// Userspace units apply backpressure; kernel records use non-waiting admission.
    fn may_defer(_writer: Self::Writer) -> bool {
        true
    }
    /// Keep a record owner runnable on its CPU while IRQs remain enabled.
    fn exclude_preemption() -> bool {
        false
    }
    fn restore_preemption(_excluded: bool) {}
    /// This port's ownership.
    fn state() -> &'static State<Self::Writer>;
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
pub struct State<W: Copy = u8> {
    deferred: defer::Deferred<W>,
    /// The next ticket to hand out.
    next: AtomicU32,
    /// The ticket advances when an owner releases or a waiter skips a stall.
    serving: AtomicU64,
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

impl<W: Copy> State<W> {
    pub const fn new() -> Self {
        Self {
            deferred: defer::Deferred::new(),
            next: AtomicU32::new(0),
            serving: AtomicU64::new(0),
            holder: AtomicU64::new(0),
            begun: AtomicU64::new(0),
            ended: AtomicU64::new(0),
            wedged: AtomicBool::new(false),
            capturing: AtomicU32::new(0),
        }
    }
}

impl<W: Copy> Default for State<W> {
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
        state.serving.load(Ordering::SeqCst) as u32,
        state.ended.load(Ordering::Relaxed),
    );
    let mut since = P::counter();
    loop {
        let serving = state.serving.load(Ordering::SeqCst) as u32;
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
            // Skip a stalled ticket unless another waiter already advanced it.
            let _ = state.serving.compare_exchange(
                serving as u64,
                serving.wrapping_add(1) as u64,
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
    parked: bool,
    port: PhantomData<P>,
}

impl<P: Port> Ownership<P> {
    /// The ticket this CPU already owns, if any.
    fn reentered(tag: u32) -> Option<u32> {
        let state = P::state();
        let holder = state.holder.load(Ordering::Relaxed);
        let ticket = (holder >> 32) as u32;
        (holder as u32 == tag && state.serving.load(Ordering::SeqCst) as u32 == ticket)
            .then_some(ticket)
    }

    fn new(saved: u64, tag: u32, ticket: u32, release: bool) -> Self {
        Self {
            saved,
            tag,
            ticket,
            release,
            puts: 0,
            parked: false,
            port: PhantomData,
        }
    }

    /// Non-fatal writers defer immediately while a record is in progress.
    pub fn acquire() -> Self {
        let saved = P::mask_interrupts();
        if P::state().deferred.active() {
            return Self::new(saved, P::cpu_tag(), 0, false).park();
        }
        P::restore_interrupts(saved);
        Self::acquire_uart()
    }

    fn park(mut self) -> Self {
        P::restore_interrupts(self.saved);
        self.parked = true;
        self
    }

    fn acquire_uart() -> Self {
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
        if state.deferred.active() {
            P::restore_interrupts(saved);
            return None;
        }
        let tag = P::cpu_tag();
        if let Some(ticket) = Self::reentered(tag) {
            return Some(Self::new(saved, tag, ticket, false));
        }
        // With no ticket outstanding, NEXT equals SERVING and the next ticket owns at once.
        let serving = state.serving.load(Ordering::SeqCst) as u32;
        if state
            .next
            .compare_exchange(
                serving,
                serving.wrapping_add(1),
                Ordering::Acquire,
                Ordering::Relaxed,
            )
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
        P::state().serving.load(Ordering::SeqCst) as u32 == self.ticket
    }

    /// Release hardware ownership and restore interrupts before record transmission.
    fn suspend(&mut self) {
        if self.parked {
            return;
        }
        if self.release {
            let _ = P::state().serving.compare_exchange(
                self.ticket as u64,
                self.ticket.wrapping_add(1) as u64,
                Ordering::SeqCst,
                Ordering::Relaxed,
            );
        }
        self.release = false;
        self.parked = true;
        P::restore_interrupts(self.saved);
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
            let state = P::state();
            let serving = state.serving.load(Ordering::SeqCst);
            if serving as u32 == self.ticket {
                let _ = state.serving.compare_exchange(
                    serving,
                    self.ticket.wrapping_add(1) as u64,
                    Ordering::SeqCst,
                    Ordering::Relaxed,
                );
            }
        }
        if !self.parked {
            P::restore_interrupts(self.saved);
        }
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
fn put<P: Port, const TEE: bool>(
    writer: P::Writer,
    byte: u8,
    ownership: &mut Ownership<P>,
) -> bool {
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

/// Transmit a non-fatal unit without owning the UART or masking interrupts for
/// its duration. Other non-fatal units are copied to the bounded deferred ring.
pub fn write_atomic<P: Port, const TEE: bool>(
    writer: P::Writer,
    bytes: &[u8],
    fresh: bool,
    ownership: &mut Ownership<P>,
) {
    if bytes.is_empty() {
        return;
    }
    struct Record<P: Port>(bool, PhantomData<P>);
    impl<P: Port> Drop for Record<P> {
        fn drop(&mut self) {
            P::restore_preemption(self.0);
        }
    }
    let _record = Record::<P>(P::exclude_preemption(), PhantomData);
    ownership.suspend();
    let state = P::state();
    loop {
        let saved = P::mask_interrupts();
        let admission = state
            .deferred
            .submit(writer, bytes, TEE, fresh, P::may_defer(writer));
        P::restore_interrupts(saved);
        match admission {
            defer::Admission::Start => break,
            defer::Admission::Queued => return,
            defer::Admission::Busy => core::hint::spin_loop(),
        }
    }
    // A skipped raw writer may have a byte already inside the hardware access.
    // Wait with interrupts restored; fatal output uses the separate fatal entry point.
    while state.begun.load(Ordering::SeqCst) != state.ended.load(Ordering::SeqCst) {
        core::hint::spin_loop();
    }
    transmit::<P, TEE>(writer, bytes, fresh);
    let mut pending = [0u8; 256];
    loop {
        let saved = P::mask_interrupts();
        let next = state.deferred.next(&mut pending);
        P::restore_interrupts(saved);
        match next {
            None => core::hint::spin_loop(),
            Some(defer::Next::Record(meta, len)) => {
                if meta.tee {
                    transmit::<P, true>(meta.writer, &pending[..len], meta.fresh);
                } else {
                    transmit::<P, false>(meta.writer, &pending[..len], meta.fresh);
                }
            }
            Some(defer::Next::Overflow(lost)) => {
                let mut report = [0u8; 96];
                transmit::<P, true>(writer, defer::overflow_line(lost, &mut report), true);
            }
            Some(defer::Next::Empty) => break,
        }
    }
}

fn transmit<P: Port, const TEE: bool>(writer: P::Writer, bytes: &[u8], fresh: bool) {
    let mut first = true;
    for chunk in bytes.chunks(16) {
        let mut ownership = Ownership::<P>::acquire_uart();
        if first {
            let lead = if fresh && P::line_open() {
                &b"\r\n"[..]
            } else {
                P::lead(writer, bytes)
            };
            put_all::<P, TEE>(writer, lead, &mut ownership);
            first = false;
        }
        put_all::<P, TEE>(writer, chunk, &mut ownership);
    }
}

/// claim-lint:ok: #2424 W25 defines the fatal contract; tests/uart_owner.rs pauses a record at the UART and requires immediate fatal bytes.
/// Fatal output never waits for another CPU or for record progress. It may
/// interrupt a unit, including when the interrupted CPU will never return.
pub fn fatal<P: Port>(bytes: &[u8]) {
    let saved = P::mask_interrupts();
    let state = P::state();
    let ticket = state.next.fetch_add(1, Ordering::Relaxed);
    state.serving.store(ticket as u64, Ordering::SeqCst);
    let ownership = Ownership::<P>::new(saved, P::cpu_tag(), ticket, true);
    for &byte in bytes {
        wait_ready::<P>();
        P::write(byte, &ownership);
        capture::<P>(byte, ownership.tag);
    }
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
    if ownership.parked {
        write_atomic::<P, TEE>(writer, bytes, kind == Kind::Line, ownership);
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
            // Defer a raw writer's remainder behind a unit already on the wire.
            if P::state().deferred.active() {
                write_atomic::<P, TEE>(writer, &bytes[index..], false, ownership);
                return;
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
