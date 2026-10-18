//! Stdin ring buffer for keyboard input
//!
//! This module provides a kernel-side ring buffer for stdin input.
//! Characters from the keyboard interrupt handler are pushed here,
//! and userspace processes can read from it via the read() syscall.

use crate::task::waitqueue::{PrepareOutcome, WaitQueueHead};
use alloc::collections::VecDeque;
#[cfg(target_arch = "aarch64")]
use core::sync::atomic::{AtomicBool, Ordering};
use spin::Mutex;

/// Console/Tty readers share the live input ring, but not the legacy stdin registry.
pub(crate) static INPUT_READERS: WaitQueueHead = WaitQueueHead::new();

/// Default stdin buffer size
pub const STDIN_BUF_SIZE: usize = 4096;

/// Thread IDs blocked waiting for stdin input
static BLOCKED_READERS: Mutex<VecDeque<u64>> = Mutex::new(VecDeque::new());

/// The global stdin buffer
#[cfg(target_arch = "aarch64")]
static STDIN_BUFFER: StdinBufferLock = StdinBufferLock(Mutex::new(StdinBuffer::new()));

#[cfg(not(target_arch = "aarch64"))]
static STDIN_BUFFER: Mutex<StdinBuffer> = Mutex::new(StdinBuffer::new());

#[path = "stdin_buffer.rs"]
mod buffer;
use buffer::StdinBuffer;

/// An ARM64 UART IRQ that finds stdin busy leaves RX masked and its bytes
/// unread. Every holder rearms RX after releasing the ring, including readers
/// and observers: the IRQ can then retry without spinning on this CPU.
#[cfg(target_arch = "aarch64")]
struct StdinBufferLock(Mutex<StdinBuffer>);

#[cfg(target_arch = "aarch64")]
static SERIAL_RX_DEFERRED: AtomicBool = AtomicBool::new(false);

#[cfg(target_arch = "aarch64")]
struct StdinGuard<'a> {
    guard: Option<spin::MutexGuard<'a, StdinBuffer>>,
    rearm_rx: bool,
}

#[cfg(target_arch = "aarch64")]
impl StdinBufferLock {
    fn lock(&self) -> StdinGuard<'_> {
        StdinGuard::new(self.0.lock())
    }

    fn try_lock(&self) -> Option<StdinGuard<'_>> {
        self.0.try_lock().map(StdinGuard::new)
    }
}

#[cfg(target_arch = "aarch64")]
impl<'a> StdinGuard<'a> {
    fn new(guard: spin::MutexGuard<'a, StdinBuffer>) -> Self {
        Self {
            guard: Some(guard),
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
        if self.rearm_rx && SERIAL_RX_DEFERRED.swap(false, Ordering::AcqRel) {
            crate::serial_aarch64::resume_rx_interrupt();
        }
    }
}

/// Acquire room before consuming the UART. Busy/full stdin leaves bytes in
/// the hardware FIFO, with RX masked until a ring holder releases it.
#[cfg(target_arch = "aarch64")]
pub(crate) fn receive_serial_from_irq(mut next: impl FnMut() -> Option<u8>) {
    crate::serial_aarch64::disable_rx_interrupt();
    // Complete the device mask before another CPU can observe the handoff
    // and rearm it; the atomic orders normal memory, not a posted MMIO write.
    // SAFETY: this barrier accesses no memory through Rust pointers.
    unsafe {
        core::arch::asm!("dsb sy", options(nostack, preserves_flags));
    }
    // Publish after masking: if a holder has already dropped, this attempt
    // acquires the ring and its own guard performs the rearm instead.
    SERIAL_RX_DEFERRED.store(true, Ordering::Release);
    let Some(mut buffer) = STDIN_BUFFER.try_lock() else {
        return;
    };
    let mut received = false;
    while !buffer.is_full() {
        let Some(byte) = next() else {
            crate::serial_aarch64::clear_rx_interrupt();
            drop(buffer);
            if received {
                wake_blocked_readers_try();
            }
            return;
        };
        assert!(buffer.push_byte(byte));
        received = true;
        INPUT_READERS.wake_up_deferred();
    }
    // No room: retain the FIFO and its pending RX/timeout indication. The next
    // reader (or flush) rearms RX after releasing the ring.
    buffer.rearm_rx = false;
    drop(buffer);
    if received {
        wake_blocked_readers_try();
    }
}

/// Deterministic #1803 case: a byte delivered while stdin is held must still
/// be available when the receiver retries, in its original order.
#[cfg(all(target_arch = "aarch64", feature = "boot_tests"))]
pub(crate) fn test_serial_contention() -> bool {
    let input = *b"echo console-v1 tty on screen\n";
    let mut next_byte = 0;
    let mut fifo = || {
        let byte = input.get(next_byte).copied();
        next_byte += usize::from(byte.is_some());
        byte
    };
    let held = STDIN_BUFFER.lock();
    receive_serial_from_irq(&mut fifo);
    drop(held);
    // The held-lock attempt must not have consumed even one byte.
    if next_byte != 0 {
        return false;
    }
    receive_serial_from_irq(|| {
        let byte = input.get(next_byte).copied();
        next_byte += usize::from(byte.is_some());
        byte
    });
    let mut output = [0; 30];
    let read = read_bytes(&mut output);
    let ordered =
        next_byte == input.len() && read == Ok(input.len()) && output[..input.len()] == input;
    {
        let mut ring = STDIN_BUFFER.lock();
        for _ in 0..STDIN_BUF_SIZE - 1 {
            assert!(ring.push_byte(b'x'));
        }
    }
    let mut full_consumed = 0;
    receive_serial_from_irq(|| {
        let byte = (full_consumed == 0).then_some(b'y');
        full_consumed += usize::from(byte.is_some());
        byte
    });
    flush_input();
    let passed = ordered && full_consumed == 0;
    crate::serial_println!(
        "[STDIN_SERIAL_CONTENTION:held_consumed=0:retried={}:ordered={}:full_consumed={}]",
        next_byte,
        u64::from(ordered),
        full_consumed
    );
    passed
}

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
