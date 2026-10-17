//! Kernel log lines composed on paths that may not log.
//!
//! An interrupt path, the NetRx softirq among them, must not log: the logger
//! takes the serial port's or the log ring's lock. A notice lets such a path
//! compose its line where the event happens, with no lock, no allocation and
//! no formatting machinery, and post it to a lock-free list. The list is
//! written out from thread context only, never from an interrupt path: on x86
//! by the next log line a thread writes and by the idle loop's log drain
//! (`serial`), on aarch64 by the next record a thread writes to the UART and
//! by the idle loop (`serial_line`). Each line goes out whole, newline
//! included, on a line of its own.
//!
//! The list links [`Node`]s, which never move while posted. A static node is
//! posted through a `&'static` reference. An object that comes and goes holds
//! a [`Notice`], which owns its node on the heap, so moving the object does not
//! move the node. A node holds one line at a time and is posted again only
//! once that line is written out, so posting needs no room. Dropping a
//! [`Notice`] whose line is still posted writes out the list first, and waits
//! for a writer on another CPU that has taken it, so the list never points at
//! freed memory. Writers take the list with interrupts masked and finish it
//! before unmasking.
//!
//! Core and `alloc` only: the host test `tests/log_notice.rs` compiles this file.

use alloc::boxed::Box;
use core::cell::UnsafeCell;
use core::ptr;
use core::sync::atomic::{AtomicPtr, AtomicU8, AtomicUsize, Ordering};

/// A node's line, newline included.
const LINE: usize = 81;

const IDLE: u8 = 0;
const COMPOSING: u8 = 1;
const POSTED: u8 = 2;
const WRITTEN: u8 = 3;

/// Posted nodes, newest first.
static PENDING: AtomicPtr<Node> = AtomicPtr::new(ptr::null_mut());

/// A `fn(usize)` called with each line's length, newline included, as it is posted; 0 until
/// one is set. The kernel's output accounting counts a posted line toward its poster here, and
/// the later write-out is not counted.
static ON_POST: AtomicUsize = AtomicUsize::new(0);

/// Call `hook` with each line's length as it is posted.
pub fn set_on_post(hook: fn(usize)) {
    ON_POST.store(hook as usize, Ordering::Release);
}

/// One posted line in the list.
pub struct Node {
    state: AtomicU8,
    next: AtomicPtr<Node>,
    len: UnsafeCell<usize>,
    line: UnsafeCell<[u8; LINE]>,
}

// SAFETY: `len` and `line` are written only by the poster that moved `state`
// out of IDLE or WRITTEN, before the Release that publishes the node, and read
// only by the writer that took the node from the list with an Acquire.
unsafe impl Sync for Node {}

impl Node {
    pub const fn new() -> Self {
        Self {
            state: AtomicU8::new(IDLE),
            next: AtomicPtr::new(ptr::null_mut()),
            len: UnsafeCell::new(0),
            line: UnsafeCell::new([0; LINE]),
        }
    }

    /// Post `text` as this node's line if the node is free: never posted, or
    /// its last line already written out. False if its line is still waiting.
    /// Lock-free and allocation-free.
    pub fn post_again(&'static self, text: &str) -> bool {
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

    /// Write the line and publish the node. The caller has claimed it, and the
    /// node stays where it is until its line is written out: it is a static, or
    /// a [`Notice`]'s heap node.
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
        let mut end = text.len().min(LINE - 1 - (digits.len() - first));
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        // SAFETY: COMPOSING gives this caller the line alone (see `Sync`).
        let line = unsafe { &mut *self.line.get() };
        line[..end].copy_from_slice(&text.as_bytes()[..end]);
        let len = end + digits.len() - first;
        line[end..len].copy_from_slice(&digits[first..]);
        line[len] = b'\n';
        // SAFETY: as above.
        unsafe { *self.len.get() = len + 1 };
        self.state.store(POSTED, Ordering::Relaxed);
        let hook = ON_POST.load(Ordering::Acquire);
        if hook != 0 {
            // SAFETY: only `set_on_post` stores a nonzero value, and it stores a `fn(usize)`.
            let hook: fn(usize) = unsafe { core::mem::transmute(hook) };
            hook(len + 1);
        }

        let me = self as *const Node as *mut Node;
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

impl Default for Node {
    fn default() -> Self {
        Self::new()
    }
}

/// A line an object posts once, about itself. The node is on the heap, so the
/// object may move while the line waits.
pub struct Notice {
    node: Box<Node>,
}

impl Notice {
    pub fn new() -> Self {
        Self {
            node: Box::new(Node::new()),
        }
    }

    /// Compose `text` followed by `number` in decimal as this notice's line and
    /// post it, the first time only. Lock-free and allocation-free.
    pub fn post(&self, text: &str, number: u64) {
        if self.node.claim(IDLE) {
            self.node.compose(text, Some(number));
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
        while self.node.state.load(Ordering::Acquire) == POSTED {
            crate::serial::write_notices();
            core::hint::spin_loop();
        }
    }
}

/// Some node is waiting to be written.
pub fn pending() -> bool {
    !PENDING.load(Ordering::Relaxed).is_null()
}

/// Hand each posted node's line, newline included, to `write`, oldest first.
/// The caller masks interrupts across the call, so a node it has taken is
/// written before anything else runs on its CPU.
pub fn take(mut write: impl FnMut(&[u8])) {
    if !pending() {
        return;
    }
    let mut newest = PENDING.swap(ptr::null_mut(), Ordering::Acquire);
    let mut oldest = ptr::null_mut();
    while !newest.is_null() {
        // SAFETY: a posted node is a static, or a Notice's heap node, which
        // Notice::drop frees only once it is WRITTEN.
        let node = unsafe { &*newest };
        let next = node.next.load(Ordering::Relaxed);
        node.next.store(oldest, Ordering::Relaxed);
        oldest = newest;
        newest = next;
    }
    while !oldest.is_null() {
        // SAFETY: as above; this caller took it from the list.
        let node = unsafe { &*oldest };
        let next = node.next.load(Ordering::Relaxed);
        // SAFETY: published by the poster's Release, taken with the Acquire above.
        let line: &[u8; LINE] = unsafe { &*node.line.get() };
        // SAFETY: as above.
        let len = unsafe { *node.len.get() };
        write(&line[..len]);
        node.state.store(WRITTEN, Ordering::Release);
        oldest = next;
    }
}

/// Write a line through `try_write`, which tries the port once without
/// waiting and reports whether it wrote the line, from a path that must not
/// wait indefinitely for the port. After `attempts` failed tries the line is
/// posted, whole, to a free node of `spare`, and goes out with the other
/// notices instead of being written into the port's current holder's output.
///
/// The second half of `spare` is kept for boot-path stage lines (`stage`). An
/// ordinary line that finds the first half waiting to be written is dropped; a
/// stage line that finds every node waiting keeps trying the port and the nodes.
pub fn write_or_post(
    spare: &'static [Node],
    line: &str,
    stage: bool,
    attempts: u32,
    mut try_write: impl FnMut() -> bool,
) {
    let usable = if stage {
        spare
    } else {
        &spare[..spare.len() / 2]
    };
    let mut tried = 0u32;
    loop {
        if try_write() {
            return;
        }
        if tried < attempts {
            tried += 1;
        } else if usable.iter().any(|node| node.post_again(line)) || !stage {
            return;
        }
        core::hint::spin_loop();
    }
}
