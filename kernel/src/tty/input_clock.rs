//! Console idle expiry follows input arriving at TTY 0, including control bytes.
//! Output, echo and serial capture activity never advance this clock.
use super::input_activity::InputClock;

static CLOCK: InputClock = InputClock::new();

pub fn record_input() {
    CLOCK.input(crate::time::get_monotonic_time());
}

/// Called once when the production x86 boot target selects the console.
pub fn start(minutes: u16) {
    CLOCK.arm(crate::time::get_monotonic_time(), minutes);
    if crate::task::kthread::kthread_run(idle_worker, "console-idle").is_err() {
        panic!("cannot start console input clock");
    }
}

fn idle_worker() {
    let sleeper = crate::task::completion::Completion::new();
    loop {
        crate::per_cpu::preempt_disable();
        let _ = sleeper.wait_timeout_uninterruptible(1, 1_000_000_000);
        crate::per_cpu::preempt_enable();
        if CLOCK.expire(crate::time::get_monotonic_time()) {
            // Thread context only: the input path stores a timestamp, never logs.
            crate::serial_println!("[console] input idle {} min", CLOCK.minutes());
            return;
        }
    }
}

