//! Aarch64 UART ownership and bounded deferred records (issue 847).
//!
//! Ownership is independent of the logger mutex. A masked caller attempts
//! one CAS, then publishes a complete record to its CPU's staging ring.
//! Only the reporter drains slots; no producer waits for a consumer.
use core::cell::UnsafeCell;
use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

const CAPACITY: usize = 1024;
const SLOTS: usize = 64;
const CPUS: usize = crate::arch_impl::aarch64::constants::MAX_CPUS;
static UART_OWNED: AtomicBool = AtomicBool::new(false);
#[cfg(feature = "boot_tests")]
pub(crate) static ORACLE_COMPLETED: [AtomicUsize; CPUS] = [const { AtomicUsize::new(0) }; CPUS];

pub static DROPPED: AtomicU64 = AtomicU64::new(0);

struct Record {
    bytes: [MaybeUninit<u8>; CAPACITY],
    len: usize,
    tee: bool,
}
impl Record {
    fn as_bytes(&self) -> &[u8] {
        // Line::char and publish initialize this prefix before publishing len.
        unsafe { core::slice::from_raw_parts(self.bytes.as_ptr().cast::<u8>(), self.len) }
    }

    const fn new() -> Self {
        Self {
            bytes: [MaybeUninit::uninit(); CAPACITY],
            len: 0,
            tee: false,
        }
    }
}
struct Slot {
    // FREE -> WRITING -> READY -> READING -> FREE. Acquires pair with
    // releases before accessing the payload, including reuse after drain.
    state: AtomicUsize,
    record: UnsafeCell<Record>,
}
// Payload access requires exclusive ownership of the slot's state.
unsafe impl Sync for Slot {}
impl Slot {
    const fn new() -> Self {
        Self {
            state: AtomicUsize::new(0),
            record: UnsafeCell::new(Record::new()),
        }
    }
}
struct Ring {
    next: AtomicUsize,
    slots: [Slot; SLOTS],
}
impl Ring {
    const fn new() -> Self {
        Self {
            next: AtomicUsize::new(0),
            slots: [const { Slot::new() }; SLOTS],
        }
    }
}
static RINGS: [Ring; CPUS] = [const { Ring::new() }; CPUS];

pub(crate) struct Ticket {
    owned: (),
}
impl Ticket {
    fn acquire() -> Option<Self> {
        UART_OWNED
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .ok()
            .map(|_| Self { owned: () })
    }
}
impl Drop for Ticket {
    fn drop(&mut self) {
        let () = self.owned;
        UART_OWNED.store(false, Ordering::Release);
    }
}

// This is the UART byte sink. The borrow requires ownership for the full
// record. Its callees use hardware I/O and atomic capture publication.
fn emit_owned<const TEE: bool>(record: &Record, ticket: &Ticket) {
    #[cfg(feature = "boot_tests")]
    let oracle = record.as_bytes().starts_with(b"[SERIAL_INTERLEAVE:");
    for &byte in record.as_bytes() {
        #[cfg(feature = "boot_tests")]
        if oracle {
            for _ in 0..64 {
                core::hint::spin_loop();
            }
        }
        crate::serial_aarch64::hardware_byte(byte, ticket);
        if TEE {
            crate::graphics::log_capture::capture_byte(byte);
            crate::log_buffer::capture_byte(byte);
        }
    }
    #[cfg(feature = "boot_tests")]
    if oracle {
        if let Some(cpu @ b'0'..=b'7') = record.as_bytes().get(b"[SERIAL_INTERLEAVE:cpu=".len()) {
            if let Some(completed) = ORACLE_COMPLETED.get(usize::from(*cpu - b'0')) {
                completed.fetch_add(1, Ordering::Release);
            }
        }
    }
}

fn publish(record: &Record) {
    // MPIDR is valid even before per-CPU software state is initialized.
    let mpidr: u64;
    unsafe {
        core::arch::asm!("mrs {}, mpidr_el1", out(reg) mpidr, options(nomem, nostack));
    }
    let ring = &RINGS[(mpidr as usize & 0xff) % CPUS];
    let first = ring.next.fetch_add(1, Ordering::Relaxed);
    for offset in 0..SLOTS {
        let slot = &ring.slots[(first.wrapping_add(offset)) % SLOTS];
        if slot
            .state
            .compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
        {
            // State WRITING belongs to this producer, including under nested
            // exceptions; another producer must claim a different slot.
            unsafe {
                let dest = &mut *slot.record.get();
                // Record's private length is bounded by Line::char. Copy only
                // the initialized prefix; do not introduce a bounds-panic path
                // from interrupt diagnostics into the panic formatter.
                core::ptr::copy_nonoverlapping(
                    record.bytes.as_ptr(),
                    dest.bytes.as_mut_ptr(),
                    record.len,
                );
                dest.len = record.len;
                dest.tee = record.tee;
            }
            slot.state.store(2, Ordering::Release);
            return;
        }
    }
    DROPPED.fetch_add(1, Ordering::Relaxed);
}

fn submit<const TEE: bool>(record: &Record) {
    if record.len == 0 {
        return;
    }
    // Treat already-masked callers conservatively as interrupt callers.
    // No per-CPU access is needed during early boot.
    let can_spin = crate::arch_interrupts_enabled() && !crate::per_cpu::in_interrupt();
    crate::arch_impl::aarch64::cpu::without_interrupts(|| {
        let attempts = if can_spin { 256 } else { 1 };
        for _ in 0..attempts {
            if let Some(ticket) = Ticket::acquire() {
                emit_owned::<TEE>(record, &ticket);
                return;
            }
            core::hint::spin_loop();
        }
        publish(record);
    });
}

/// Stack assembly. Oversize records are dropped as a unit, not split into
/// independently owned pieces. Newline ends a record; drop flushes a caller's
/// unterminated output unit (TTY prompts and single-character diagnostics).
pub type Line = Buffer<false>;
pub type TeeLine = Buffer<true>;

pub struct Buffer<const TEE: bool> {
    record: Record,
    overflow: bool,
}
impl<const TEE: bool> Buffer<TEE> {
    pub const fn new() -> Self {
        Self {
            record: Record {
                tee: TEE,
                ..Record::new()
            },
            overflow: false,
        }
    }
    pub fn char(&mut self, byte: u8) {
        if self.record.len < CAPACITY {
            self.record.bytes[self.record.len].write(byte);
            self.record.len += 1;
        } else {
            self.overflow = true;
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
        if self.overflow {
            DROPPED.fetch_add(1, Ordering::Relaxed);
        } else {
            submit::<TEE>(&self.record);
        }
        self.record.len = 0;
        self.overflow = false;
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

/// Thread-context reporter, started after scheduler initialization.
pub fn start_reporter() {
    crate::task::kthread::kthread_run(
        || loop {
            drain();
            let (seconds, nanos) = crate::time::get_monotonic_time_ns();
            let deadline = seconds
                .saturating_mul(1_000_000_000)
                .saturating_add(nanos)
                .saturating_add(10_000_000);
            crate::task::scheduler::with_scheduler(|sched| {
                sched.block_current_for_timer(deadline);
            });
            crate::task::scheduler::yield_current();
        },
        "uart-reporter",
    )
    .expect("UART reporter creation failed");
}

pub fn drain() {
    for ring in &RINGS {
        for slot in &ring.slots {
            if slot.state.load(Ordering::Acquire) != 2 {
                continue;
            }
            crate::arch_impl::aarch64::cpu::without_interrupts(|| {
                let Some(ticket) = Ticket::acquire() else {
                    return;
                };
                if slot
                    .state
                    .compare_exchange(2, 3, Ordering::Acquire, Ordering::Relaxed)
                    .is_ok()
                {
                    unsafe {
                        let record = &*slot.record.get();
                        if record.tee {
                            emit_owned::<true>(record, &ticket);
                        } else {
                            emit_owned::<false>(record, &ticket);
                        }
                    }
                    slot.state.store(0, Ordering::Release);
                }
            });
        }
    }
}
