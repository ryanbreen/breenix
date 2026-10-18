//! Stdin ring buffer for keyboard input
//!
//! This module provides a kernel-side ring buffer for stdin input.
//! Characters from the keyboard interrupt handler are pushed here,
//! and userspace processes can read from it via the read() syscall.

use crate::task::waitqueue::{PrepareOutcome, WaitQueueHead};
use alloc::collections::VecDeque;
#[cfg(target_arch = "aarch64")]
use core::sync::atomic::{fence, AtomicBool, Ordering};
use spin::Mutex;

/// Console/Tty readers share the live input ring, but not the legacy stdin registry.
pub(crate) static INPUT_READERS: WaitQueueHead = WaitQueueHead::new();

/// Default stdin buffer size
pub const STDIN_BUF_SIZE: usize = 4096;

/// Thread IDs blocked waiting for stdin input
static BLOCKED_READERS: Mutex<VecDeque<u64>> = Mutex::new(VecDeque::new());

/// The global stdin buffer
#[cfg(target_arch = "aarch64")]
static STDIN_BUFFER: StdinBufferLock = StdinBufferLock::new(SerialRx {
    mask: crate::serial_aarch64::disable_rx_interrupt,
    rearm: crate::serial_aarch64::resume_rx_interrupt,
    acknowledge_drained: crate::serial_aarch64::clear_rx_interrupt,
});

#[cfg(not(target_arch = "aarch64"))]
static STDIN_BUFFER: Mutex<StdinBuffer> = Mutex::new(StdinBuffer::new());

#[path = "stdin_buffer.rs"]
mod buffer;
use buffer::StdinBuffer;

/// The receive-interrupt controls of the UART feeding a ring. The boot test
/// substitutes a fake device and leaves the real UART alone.
#[cfg(target_arch = "aarch64")]
struct SerialRx {
    mask: fn(),
    rearm: fn(),
    /// Called once the FIFO reports empty. It must not read the receive
    /// register: a byte arriving after the empty report would be lost.
    acknowledge_drained: fn(),
}

/// An ARM64 UART IRQ that finds stdin busy leaves RX masked and its bytes
/// unread. A holder (reader, observer or the IRQ itself) rearms RX after
/// releasing the ring: the IRQ can then retry without spinning on this CPU.
#[cfg(target_arch = "aarch64")]
struct StdinBufferLock {
    ring: Mutex<StdinBuffer>,
    rx_deferred: AtomicBool,
    rx: SerialRx,
}

#[cfg(target_arch = "aarch64")]
struct StdinGuard<'a> {
    guard: Option<spin::MutexGuard<'a, StdinBuffer>>,
    lock: &'a StdinBufferLock,
    rearm_rx: bool,
}

#[cfg(target_arch = "aarch64")]
impl StdinBufferLock {
    const fn new(rx: SerialRx) -> Self {
        Self {
            ring: Mutex::new(StdinBuffer::new()),
            rx_deferred: AtomicBool::new(false),
            rx,
        }
    }

    fn lock(&self) -> StdinGuard<'_> {
        StdinGuard::new(self.ring.lock(), self)
    }

    fn try_lock(&self) -> Option<StdinGuard<'_>> {
        self.ring.try_lock().map(|guard| StdinGuard::new(guard, self))
    }

    /// Acquire room before consuming the UART. Busy/full stdin leaves bytes
    /// in the hardware FIFO, with RX masked until a ring holder releases it.
    fn receive_serial(&self, mut next: impl FnMut() -> Option<u8>) {
        (self.rx.mask)();
        // Complete the device mask before another CPU can observe the handoff
        // and rearm it; the atomic orders normal memory, not a posted MMIO
        // write.
        // SAFETY: this barrier accesses no memory through Rust pointers.
        unsafe {
            core::arch::asm!("dsb sy", options(nostack, preserves_flags));
        }
        // Publish after masking: if a holder has already dropped, this attempt
        // acquires the ring and its own guard performs the rearm instead. The
        // fence pairs with the one in StdinGuard::drop: of this store and the
        // holder's unlock, each side sees the other's (store buffering).
        self.rx_deferred.store(true, Ordering::Relaxed);
        fence(Ordering::SeqCst);
        let Some(mut buffer) = self.try_lock() else {
            return;
        };
        let mut received = false;
        while !buffer.is_full() {
            let Some(byte) = next() else {
                (self.rx.acknowledge_drained)();
                drop(buffer);
                if received {
                    wake_blocked_readers_try();
                }
                return;
            };
            // Cannot fail: the loop condition reserved the room.
            let pushed = buffer.push_byte(byte);
            debug_assert!(pushed);
            received = true;
            INPUT_READERS.wake_up_deferred();
        }
        // No room: retain the FIFO and its pending RX/timeout indication. The
        // next reader (or flush) rearms RX after releasing the ring.
        buffer.rearm_rx = false;
        drop(buffer);
        if received {
            wake_blocked_readers_try();
        }
    }
}

#[cfg(target_arch = "aarch64")]
impl<'a> StdinGuard<'a> {
    fn new(guard: spin::MutexGuard<'a, StdinBuffer>, lock: &'a StdinBufferLock) -> Self {
        Self {
            guard: Some(guard),
            lock,
            rearm_rx: true,
        }
    }
}

#[cfg(target_arch = "aarch64")]
impl core::ops::Deref for StdinGuard<'_> {
    type Target = StdinBuffer;
    fn deref(&self) -> &StdinBuffer {
        self.guard.as_ref().unwrap()
    }
}

#[cfg(target_arch = "aarch64")]
impl core::ops::DerefMut for StdinGuard<'_> {
    fn deref_mut(&mut self) -> &mut StdinBuffer {
        self.guard.as_mut().unwrap()
    }
}

#[cfg(target_arch = "aarch64")]
impl Drop for StdinGuard<'_> {
    fn drop(&mut self) {
        drop(self.guard.take());
        if self.rearm_rx {
            // Pairs with the fence in receive_serial.
            fence(Ordering::SeqCst);
            if self.lock.rx_deferred.swap(false, Ordering::AcqRel) {
                (self.lock.rx.rearm)();
            }
        }
    }
}

/// The ARM64 UART interrupt's receiver for the live stdin ring.
#[cfg(target_arch = "aarch64")]
pub(crate) fn receive_serial_from_irq(next: impl FnMut() -> Option<u8>) {
    STDIN_BUFFER.receive_serial(next);
}

#[cfg(all(target_arch = "aarch64", feature = "boot_tests"))]
mod contention_test {
    use super::{SerialRx, StdinBufferLock, STDIN_BUF_SIZE};
    use core::sync::atomic::{AtomicU64, Ordering};

    static MASKS: AtomicU64 = AtomicU64::new(0);
    static REARMS: AtomicU64 = AtomicU64::new(0);
    static ACKS: AtomicU64 = AtomicU64::new(0);

    /// A private ring fed by a fake UART: the live ring and device are
    /// untouched, so real input can neither disturb nor be lost to the test.
    static RING: StdinBufferLock = StdinBufferLock::new(SerialRx {
        mask: || {
            MASKS.fetch_add(1, Ordering::Relaxed);
        },
        rearm: || {
            REARMS.fetch_add(1, Ordering::Relaxed);
        },
        acknowledge_drained: || {
            ACKS.fetch_add(1, Ordering::Relaxed);
        },
    });

    /// Deterministic #1803 case: a byte delivered while stdin is held must
    /// still be available when the receiver retries, in its original order,
    /// and a full ring leaves the FIFO unread and RX masked until a reader
    /// releases room.
    pub(crate) fn run() -> bool {
        let input = *b"echo console-v1 tty on screen\n";
        let next_byte = core::cell::Cell::new(0);
        let fifo = || {
            let byte = input.get(next_byte.get()).copied();
            next_byte.set(next_byte.get() + usize::from(byte.is_some()));
            byte
        };

        // Leg 1: the ring is held. The FIFO is left unread; releasing the
        // ring rearms RX.
        let held = RING.lock();
        let masks = MASKS.load(Ordering::Relaxed);
        RING.receive_serial(fifo);
        let masked = MASKS.load(Ordering::Relaxed) == masks + 1;
        let held_consumed = next_byte.get();
        let rearms = REARMS.load(Ordering::Relaxed);
        drop(held);
        let release_rearmed = REARMS.load(Ordering::Relaxed) == rearms + 1;

        // Leg 2: the retry drains the FIFO in order and acknowledges it once.
        let acks = ACKS.load(Ordering::Relaxed);
        RING.receive_serial(fifo);
        let retry_acked = ACKS.load(Ordering::Relaxed) == acks + 1;
        let retry_consumed = next_byte.get() - held_consumed;
        let mut output = [0; 30];
        let read = RING.lock().read_bytes(&mut output);
        let ordered = held_consumed == 0
            && retry_consumed == input.len()
            && read == input.len()
            && output[..input.len()] == input;

        // Leg 3: a full ring leaves the FIFO unread and unacknowledged and RX
        // masked; the reader that makes room rearms RX.
        let mut filled = true;
        {
            let mut ring = RING.lock();
            for _ in 0..STDIN_BUF_SIZE - 1 {
                filled &= ring.push_byte(b'x');
            }
        }
        let mut full_consumed = 0;
        let acks = ACKS.load(Ordering::Relaxed);
        let rearms = REARMS.load(Ordering::Relaxed);
        RING.receive_serial(|| {
            let byte = (full_consumed == 0).then_some(b'y');
            full_consumed += usize::from(byte.is_some());
            byte
        });
        let full_acks = ACKS.load(Ordering::Relaxed) - acks;
        let full_stayed_masked = REARMS.load(Ordering::Relaxed) == rearms;
        RING.lock().flush();
        let reader_rearmed = REARMS.load(Ordering::Relaxed) == rearms + 1;

        let passed = masked
            && release_rearmed
            && retry_acked
            && ordered
            && filled
            && full_consumed == 0
            && full_acks == 0
            && full_stayed_masked
            && reader_rearmed;
        crate::serial_println!(
            "[STDIN_SERIAL_CONTENTION:held_consumed={}:release_rearmed={}:retry_consumed={}:ordered={}:full_consumed={}:full_acks={}:full_stayed_masked={}:reader_rearmed={}]",
            held_consumed,
            u64::from(release_rearmed),
            retry_consumed,
            u64::from(ordered),
            full_consumed,
            full_acks,
            u64::from(full_stayed_masked),
            u64::from(reader_rearmed)
        );
        passed
    }
}

#[cfg(all(target_arch = "aarch64", feature = "boot_tests"))]
pub(crate) use contention_test::run as test_serial_contention;

/// Push a byte to the stdin buffer
/// Called from keyboard task when a character is typed
///
/// Note: With TTY integration, keyboard input now goes through the TTY layer
/// which handles echo, line editing, and signals. This function is kept for
/// fallback when TTY is not initialized.
#[allow(dead_code)]
pub fn push_byte(byte: u8) {
    let mut buffer = STDIN_BUFFER.lock();
    if buffer.push_byte(byte) {
        INPUT_READERS.wake_up_deferred();
        // Echo character to serial output for now
        crate::serial::write_byte(crate::serial::Writer::Tty, byte);

        // Drop buffer lock before waking readers to avoid deadlock
        drop(buffer);

        // Wake any blocked readers
        wake_blocked_readers();
    } else {
        log::warn!("stdin buffer full, dropping byte: 0x{:02x}", byte);
    }
}

/// Push a byte to stdin from interrupt context (uses try_lock to avoid deadlock)
/// Returns true if the byte was pushed, false if locks couldn't be acquired
///
/// Note: This function does NOT echo. Echo is handled by the TTY layer
/// which respects termios settings (ECHO flag).
pub fn push_byte_from_irq(byte: u8) -> bool {
    // Try to acquire the buffer lock - don't block in interrupt context
    if let Some(mut buffer) = STDIN_BUFFER.try_lock() {
        if buffer.push_byte(byte) {
            INPUT_READERS.wake_up_deferred();
            drop(buffer);

            // Try to wake blocked readers (may fail if scheduler lock is held)
            wake_blocked_readers_try();
            return true;
        }
    }
    false
}

/// Publish a canonical EOF boundary without inserting a userspace byte.
pub fn push_eof_from_irq() -> bool {
    if let Some(mut buffer) = STDIN_BUFFER.try_lock() {
        if buffer.push_eof() {
            INPUT_READERS.wake_up_deferred();
            drop(buffer);
            wake_blocked_readers_try();
            return true;
        }
    }
    false
}

/// Flush type-ahead already transferred from the console line discipline.
pub fn flush_input() {
    crate::arch_without_interrupts(|| STDIN_BUFFER.lock().flush());
}

/// Try to wake blocked readers without blocking (for interrupt context)
#[cfg(target_arch = "x86_64")]
fn wake_blocked_readers_try() {
    let readers: VecDeque<u64> = {
        if let Some(mut blocked) = BLOCKED_READERS.try_lock() {
            core::mem::take(&mut *blocked)
        } else {
            return; // Can't get lock, readers will be woken when they retry
        }
    };

    if readers.is_empty() {
        return;
    }

    // Each under the lock of the CPU that owns it, or held for that CPU
    // (`unblock_thread`): never the whole scheduler (#1649).
    for thread_id in &readers {
        let _ = crate::task::scheduler::unblock_thread(*thread_id);
    }

    // Trigger reschedule so the woken thread runs soon
    crate::task::scheduler::set_need_resched();
}

/// Wake blocked readers on ARM64 (non-blocking version for interrupt context)
#[cfg(target_arch = "aarch64")]
fn wake_blocked_readers_try() {
    let readers: VecDeque<u64> = {
        if let Some(mut blocked) = BLOCKED_READERS.try_lock() {
            core::mem::take(&mut *blocked)
        } else {
            return; // Can't get lock, readers will be woken when they retry
        }
    };

    if readers.is_empty() {
        return;
    }

    // Try to wake threads via the scheduler
    crate::task::scheduler::with_scheduler(|sched| {
        for thread_id in &readers {
            sched.unblock(*thread_id);
        }
    });

    // Trigger reschedule so the woken thread runs soon
    crate::task::scheduler::set_need_resched();
}

/// Read bytes from stdin buffer
/// Returns Ok(n) with bytes read, or Err(EAGAIN) if would block
pub fn read_bytes(buf: &mut [u8]) -> Result<usize, i32> {
    let mut buffer = STDIN_BUFFER.lock();

    if buffer.is_empty() {
        // No data available - would block
        return Err(11); // EAGAIN
    }

    Ok(buffer.read_bytes(buf))
}

/// Check if stdin has data available
#[allow(dead_code)]
pub fn has_data() -> bool {
    !STDIN_BUFFER.lock().is_empty()
}

/// Register a thread as waiting for stdin input
///
/// Used by sys_read when blocking on stdin. When data becomes available
/// via push_byte_from_irq, the blocked readers are woken.
pub fn register_blocked_reader(thread_id: u64) -> Result<(), i32> {
    let mut blocked = BLOCKED_READERS.lock();
    if !blocked.contains(&thread_id) {
        blocked.try_reserve(1).map_err(|_| 12)?;
        blocked.push_back(thread_id);
        log::trace!("stdin: Thread {} blocked waiting for input", thread_id);
    }
    Ok(())
}

/// Unregister a thread from waiting for stdin (e.g., on signal or timeout)
pub fn unregister_blocked_reader(thread_id: u64) {
    let mut blocked = BLOCKED_READERS.lock();
    blocked.retain(|&id| id != thread_id);
}

/// Wake all threads blocked on stdin read
///
/// Note: With TTY integration, blocked readers are woken through
/// TtyDevice::wake_blocked_readers. This function is kept for fallback.
#[cfg(target_arch = "x86_64")]
#[allow(dead_code)]
fn wake_blocked_readers() {
    let readers: VecDeque<u64> = {
        let mut blocked = BLOCKED_READERS.lock();
        core::mem::take(&mut *blocked)
    };

    if readers.is_empty() {
        return;
    }

    log::trace!("stdin: Waking {} blocked readers", readers.len());

    // Each under the lock of the CPU that owns it, or held for that CPU
    // (`unblock_thread`): never the whole scheduler (#1649).
    for thread_id in readers {
        let _ = crate::task::scheduler::unblock_thread(thread_id);
        log::trace!("stdin: Woke thread {}", thread_id);
    }

    // Trigger reschedule to let woken threads run
    crate::task::scheduler::set_need_resched();
}

/// Wake blocked readers on ARM64
#[cfg(target_arch = "aarch64")]
#[allow(dead_code)]
fn wake_blocked_readers() {
    let readers: VecDeque<u64> = {
        let mut blocked = BLOCKED_READERS.lock();
        core::mem::take(&mut *blocked)
    };

    if readers.is_empty() {
        return;
    }

    // Wake each blocked thread
    crate::task::scheduler::with_scheduler(|sched| {
        for thread_id in readers {
            sched.unblock(thread_id);
        }
    });

    // Trigger reschedule to let woken threads run
    crate::task::scheduler::set_need_resched();
}

/// Get the number of bytes available in the stdin buffer
#[allow(dead_code)]
pub fn available() -> usize {
    STDIN_BUFFER.lock().available()
}

/// Debug helper: inject a character into stdin as if typed on the keyboard.
/// This can be called from GDB: `call debug_inject_stdin_char(0x68)` for 'h'
/// or from automation scripts to trigger keyboard input paths non-interactively.
#[no_mangle]
pub extern "C" fn debug_inject_stdin_char(byte: u64) {
    push_byte_from_irq(byte as u8);
}

/// Check/copy and publish under one ring guard with local IRQs masked.
/// The guard is dropped here before the scheduler wait lifecycle.
pub(crate) fn read_or_prepare(
    buf: &mut [u8],
    is_nonblocking: bool,
) -> Result<usize, PrepareOutcome> {
    crate::arch_without_interrupts(|| {
        let mut buffer = STDIN_BUFFER.lock();
        if !buffer.is_empty() || buf.is_empty() {
            return Ok(buffer.read_bytes(buf));
        }
        if is_nonblocking {
            return Err(PrepareOutcome::Mismatch);
        }
        Err(INPUT_READERS.prepare_to_wait_checked(
            crate::task::thread::ThreadState::BlockedOnIO,
            None,
            || buffer.is_empty(),
        ))
    })
}

/// Boot observer is read-only: it neither consumes input nor removes waiters.
#[cfg(feature = "boot_tests")]
pub(crate) fn input_witness(tid: u64) -> (bool, usize) {
    crate::arch_without_interrupts(|| {
        let buffer = STDIN_BUFFER.lock();
        (INPUT_READERS.contains_waiter(tid), buffer.len)
    })
}
