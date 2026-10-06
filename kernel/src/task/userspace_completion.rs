//! Completion reporting from a normal thread, including fault/signal exits.
use core::sync::atomic::{AtomicBool, Ordering};

static REPORTER_STARTED: AtomicBool = AtomicBool::new(false);

pub fn reporter_started() -> bool {
    REPORTER_STARTED.load(Ordering::SeqCst)
}

#[cfg(feature = "testing")]
pub fn begin_loading() {
    REPORTER_STARTED.store(true, Ordering::SeqCst);
}

#[cfg(feature = "testing")]
pub fn start() {
    if super::kthread::kthread_run(report_when_finished, "userspace-completion").is_err() {
        REPORTER_STARTED.store(false, Ordering::SeqCst);
        panic!("cannot start userspace completion reporter");
    }
}

/// One-second polls to wait, once no userspace thread remains, for the exits
/// still being committed to reach the tally.
#[cfg(feature = "testing")]
const EXIT_COMMIT_POLLS: u32 = 5;

#[cfg(feature = "testing")]
fn report_when_finished() {
    let mut commit_polls = 0;
    loop {
        let finished = super::scheduler::with_scheduler(|scheduler| {
            !scheduler.has_userspace_threads()
        }).unwrap_or(false);
        // An empty scheduler does not mean the tally is final: a fault or
        // signal death terminates the process's threads before it commits the
        // process exit the tally counts, and on SMP this thread can run in
        // between. Every published process records a start, so the report
        // waits until the exits catch up. A row removed without an exit would
        // hold that off forever, so the wait is bounded and the report's
        // `started=` against `exited=` then shows the shortfall.
        if finished {
            let (exited, _) = super::exit_tally::totals();
            if exited >= super::exit_tally::started() || commit_polls >= EXIT_COMMIT_POLLS {
                crate::syscall::handlers::report_userspace_completion();
                return;
            }
            commit_polls += 1;
        }
        let tid = super::scheduler::current_thread_id().expect("completion thread identity");
        let (seconds, nanos) = crate::time::get_monotonic_time_ns();
        let wake_at = seconds.saturating_mul(1_000_000_000).saturating_add(nanos)
            .saturating_add(1_000_000_000);
        super::scheduler::with_scheduler(|scheduler| scheduler.block_current_for_timer(wake_at));
        super::scheduler::yield_current();
        loop {
            crate::arch_halt_with_interrupts();
            let blocked = super::scheduler::with_scheduler(|scheduler| {
                scheduler.get_thread(tid).is_some_and(|thread| {
                    thread.state == super::thread::ThreadState::BlockedOnTimer
                })
            }).unwrap_or(false);
            if !blocked { break; }
        }
    }
}
