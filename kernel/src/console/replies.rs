//! Synchronous delivery of queued VT reports, outside the grid lock.
use super::{vt, with_grid};
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use spin::Mutex;

pub(super) static REPLY_QUEUED: AtomicBool = AtomicBool::new(false);
// Every holder masks interrupts before taking this lock, so a waiter cannot
// preempt its holder on the same CPU. No terminal or grid lock is held on entry.
static REPLY_DELIVERY: Mutex<()> = Mutex::new(());
static REPLIES_TAKEN: [AtomicUsize; vt::VT_COUNT] = [const { AtomicUsize::new(0) }; vt::VT_COUNT];

/// Wait for any existing delivery, then deliver the reports this write queued
/// before returning. The existing holder may already have cleared REPLY_QUEUED
/// while its bytes are still on their way to the terminal's input.
pub fn deliver_queued_replies() {
    while !deliver_pass() {
        // Input may be waiting for a preempted line-discipline holder. Let
        // it run with interrupts restored and no delivery lock held.
        crate::task::scheduler::yield_current();
    }
}

/// Called by the render thread too, for reports queued by interrupt-time echo.
/// Replies go through the input line discipline, which may echo into the grid,
/// so the grid lock must be released before handing a byte to the TTY.
pub(super) fn deliver_replies() {
    let _ = deliver_pass();
}

fn deliver_pass() -> bool {
    crate::arch_without_interrupts(|| {
        let _delivery = REPLY_DELIVERY.lock();
        REPLY_QUEUED.store(false, Ordering::SeqCst);
        for (index, taken_before) in REPLIES_TAKEN.iter().enumerate() {
            loop {
                // Do not send accepted bytes twice if their removal from
                // the grid queue encountered contention.
                let unacknowledged = taken_before.load(Ordering::Relaxed);
                if unacknowledged != 0 {
                    if with_grid(|console| console.vts[index].replies.consume(unacknowledged))
                        .is_none()
                    {
                        return false;
                    }
                    taken_before.store(0, Ordering::Relaxed);
                }
                let mut bytes = [0u8; 64];
                let Some(n) = with_grid(|console| console.vts[index].replies.peek(&mut bytes))
                else {
                    return false;
                };
                if n == 0 {
                    break;
                }
                let taken = bytes[..n]
                    .iter()
                    .take_while(|&&byte| crate::tty::driver::push_reply(index + 1, byte))
                    .count();
                if taken != 0
                    && with_grid(|console| console.vts[index].replies.consume(taken)).is_none()
                {
                    taken_before.store(taken, Ordering::Relaxed);
                }
                if taken != n {
                    return false;
                }
                // Keep going until empty, including a final short batch.
            }
        }
        !REPLY_QUEUED.load(Ordering::SeqCst)
    })
}
