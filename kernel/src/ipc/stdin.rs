//! Stdin ring buffer for keyboard input
//!
//! This module provides a kernel-side ring buffer for stdin input.
//! Characters from the keyboard interrupt handler are pushed here,
//! and userspace processes can read from it via the read() syscall.

use crate::task::waitqueue::{PrepareOutcome, WaitQueueHead};
use alloc::collections::VecDeque;
use spin::Mutex;

/// Console/Tty readers share the live input ring, but not the legacy stdin registry.
pub(crate) static INPUT_READERS: WaitQueueHead = WaitQueueHead::new();

/// Default stdin buffer size
pub const STDIN_BUF_SIZE: usize = 4096;

/// Thread IDs blocked waiting for stdin input
static BLOCKED_READERS: Mutex<VecDeque<u64>> = Mutex::new(VecDeque::new());

/// The global stdin buffer
static STDIN_BUFFER: Mutex<StdinBuffer> = Mutex::new(StdinBuffer::new());

#[path = "stdin_buffer.rs"]
mod buffer;
use buffer::StdinBuffer;

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

    // Try to wake threads via the scheduler's non-blocking path
    // Note: We use the with_scheduler variant here which may need to disable
    // interrupts, but since we're already in an interrupt handler with a
    // non-reentrant interrupt, this is safe.
    crate::task::scheduler::with_scheduler(|sched| {
        for thread_id in &readers {
            sched.unblock(*thread_id);
        }
    });

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

    // Wake each blocked thread
    crate::task::scheduler::with_scheduler(|sched| {
        for thread_id in readers {
            sched.unblock(thread_id);
            log::trace!("stdin: Woke thread {}", thread_id);
        }
    });

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
