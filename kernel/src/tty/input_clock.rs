//! Console idle expiry follows input arriving at TTY 0, including control bytes.
//! Output, echo and serial capture activity never advance this clock.
//! claim-lint:ok: the two input entry points in `kernel/src/tty/driver.rs` call record_input.
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
    let mut next_alive = 0;
    loop {
        crate::per_cpu::preempt_disable();
        let _ = sleeper.wait_timeout_uninterruptible(1, 1_000_000_000);
        crate::per_cpu::preempt_enable();
        let now = crate::time::get_monotonic_time();
        if CLOCK.expire(now) {
            // Thread context only: the input path stores a timestamp, never logs.
            // claim-lint:ok: record_input in kernel/src/tty/input_clock.rs calls only CLOCK.input and the clock read.
            crate::serial_println!("[console] input idle {} min", CLOCK.minutes());
            return;
        }
        if now >= next_alive {
            // A quiet login prompt still needs a live input clock. This serial-only
            // pulse lets the host bound a hang or an idle marker lost during SOL recovery.
            crate::serial_println!("[console] input clock alive");
            next_alive = now.saturating_add(30_000);
        }
    }
}

