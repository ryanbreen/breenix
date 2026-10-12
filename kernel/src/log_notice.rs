//! Kernel log lines composed on paths that may not log.
//!
//! An interrupt path, the NetRx softirq among them, must not log: the logger
//! takes the serial port's or the log ring's lock. A [`Notice`] lets such a
//! path compose its line where the event happens, with no lock, no allocation
//! and no formatting machinery, and post it to a lock-free list. The kernel's
//! next output writes the list out ahead of itself: on x86 the next log line
//! or the idle loop's log drain (`serial`), on aarch64 the next record any CPU
//! writes to the UART (`serial_line`).
//!
//! A notice lives in the object it reports on, or in a static, so posting needs
//! no room: a notice holds one line at a time and is posted again only once
//! that line is written out. Dropping a posted notice first writes out the
//! list, and waits for a writer on another CPU that has taken it, so the list
//! never points at freed memory. Writers take the list with interrupts masked
//! and finish it before unmasking.

use core::cell::UnsafeCell;
use core::ptr;
use core::sync::atomic::{AtomicPtr, AtomicU8, Ordering};

/// A notice's line, without its newline.
const LINE: usize = 80;

const IDLE: u8 = 0;
const COMPOSING: u8 = 1;
const POSTED: u8 = 2;
const WRITTEN: u8 = 3;

/// Posted notices, newest first.
static PENDING: AtomicPtr<Notice> = AtomicPtr::new(ptr::null_mut());

pub struct Notice {
    state: AtomicU8,
    next: AtomicPtr<Notice>,
    len: UnsafeCell<usize>,
    line: UnsafeCell<[u8; LINE]>,
}

// SAFETY: `len` and `line` are written only by the poster that moved `state`
// out of IDLE, before the Release that publishes the notice, and read only by
// the writer that took the notice from the list with an Acquire.
unsafe impl Sync for Notice {}

impl Notice {
    pub const fn new() -> Self {
        Self {
            state: AtomicU8::new(IDLE),
            next: AtomicPtr::new(ptr::null_mut()),
            len: UnsafeCell::new(0),
            line: UnsafeCell::new([0; LINE]),
        }
    }

    /// Compose `text` followed by `number` in decimal as this notice's line and
    /// post it, the first time only. Lock-free and allocation-free.
    pub fn post(&self, text: &str, number: u64) {
        if self.claim(IDLE) {
            self.compose(text, Some(number));
        }
    }

    /// Post `text` as this notice's line if the notice is free: never posted,
    /// or its last line already written out. False if its line is still
    /// waiting. Lock-free and allocation-free.
    pub fn post_again(&self, text: &str) -> bool {
        if self.claim(IDLE) || self.claim(WRITTEN) {
            self.compose(text, None);
            true
        } else {
            false
        }
    }

    fn claim(&self, from: u8) -> bool {
        self.state
            .compare_exchange(from, COMPOSING, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
    }

    /// Write the line and publish the notice. The caller has claimed it.
    fn compose(&self, text: &str, number: Option<u64>) {
        let mut digits = [0u8; 20];
        let mut first = digits.len();
        if let Some(mut rest) = number {
            loop {
                first -= 1;
                digits[first] = b'0' + (rest % 10) as u8;
                rest /= 10;
                if rest == 0 {
                    break;
                }
            }
        }
        let mut end = text.len().min(LINE - (digits.len() - first));
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        // SAFETY: COMPOSING gives this caller the line alone (see `Sync`).
        let line = unsafe { &mut *self.line.get() };
        line[..end].copy_from_slice(&text.as_bytes()[..end]);
        let len = end + digits.len() - first;
        line[end..len].copy_from_slice(&digits[first..]);
        // SAFETY: as above.
        unsafe { *self.len.get() = len };
        self.state.store(POSTED, Ordering::Relaxed);

        let me = self as *const Notice as *mut Notice;
        let mut head = PENDING.load(Ordering::Relaxed);
        loop {
            self.next.store(head, Ordering::Relaxed);
            match PENDING.compare_exchange_weak(head, me, Ordering::Release, Ordering::Relaxed) {
                Ok(_) => return,
                Err(now) => head = now,
            }
        }
    }
}

impl Default for Notice {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for Notice {
    fn drop(&mut self) {
        while self.state.load(Ordering::Acquire) == POSTED {
            crate::serial::write_notices();
            core::hint::spin_loop();
        }
    }
}

/// Some notice is waiting to be written.
pub fn pending() -> bool {
    !PENDING.load(Ordering::Relaxed).is_null()
}

/// Hand each posted notice's line to `write`, oldest first. The caller masks
/// interrupts across the call, so a notice it has taken is written before
/// anything else runs on its CPU.
pub fn take(mut write: impl FnMut(&[u8])) {
    if !pending() {
        return;
    }
    let mut newest = PENDING.swap(ptr::null_mut(), Ordering::Acquire);
    let mut oldest = ptr::null_mut();
    while !newest.is_null() {
        // SAFETY: a posted notice stays allocated until it is WRITTEN (`Drop`).
        let notice = unsafe { &*newest };
        let next = notice.next.load(Ordering::Relaxed);
        notice.next.store(oldest, Ordering::Relaxed);
        oldest = newest;
        newest = next;
    }
    while !oldest.is_null() {
        // SAFETY: as above; this caller took it from the list.
        let notice = unsafe { &*oldest };
        let next = notice.next.load(Ordering::Relaxed);
        // SAFETY: published by the poster's Release, taken with the Acquire above.
        let line: &[u8; LINE] = unsafe { &*notice.line.get() };
        // SAFETY: as above.
        let len = unsafe { *notice.len.get() };
        write(&line[..len]);
        notice.state.store(WRITTEN, Ordering::Release);
        oldest = next;
    }
}
