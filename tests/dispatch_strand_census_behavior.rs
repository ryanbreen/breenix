//! Exercise the real census with controlled scheduler samples and serial output.
// Including the production module also brings its boot-only startup APIs into
// this host harness; those APIs are intentionally not invoked here.
#![allow(dead_code)]
extern crate self as log;
#[macro_export]
macro_rules! error { ($($arg:tt)*) => { panic!($($arg)*) }; }
fn arch_interrupts_enabled() -> bool {
    true
}
fn arch_halt_with_interrupts() {}
mod time {
    pub fn get_ticks() -> u64 {
        0
    }
    pub fn get_monotonic_time_ns() -> (u64, u64) {
        (2, 0)
    }
}
mod serial {
    pub static OUTPUT: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());
    pub fn _log_print(args: std::fmt::Arguments<'_>) {
        OUTPUT.lock().unwrap().push_str(&args.to_string());
    }
    pub fn enqueue_log(args: std::fmt::Arguments<'_>) {
        _log_print(args);
    }
    pub fn drain_log_once() -> bool {
        false
    }
}
pub mod thread {
    #[derive(Clone, Copy, PartialEq, Eq)]
    pub enum ThreadState {
        Running,
        Ready,
        Blocked,
        BlockedOnSignal,
        BlockedOnChildExit,
        BlockedOnTimer,
        BlockedOnIO,
        Terminated,
    }
    impl ThreadState {
        pub fn is_blocked(self) -> bool {
            !matches!(self, Self::Running | Self::Ready | Self::Terminated)
        }
    }
}
pub mod scheduler {
    use crate::thread::ThreadState;
    pub static SAMPLE: std::sync::Mutex<Option<(ThreadState, Option<u64>)>> =
        std::sync::Mutex::new(None);
    pub fn thread_wait(_: u64) -> Option<(ThreadState, Option<u64>)> {
        *SAMPLE.lock().unwrap()
    }
    pub fn thread_state(_: u64) -> Option<ThreadState> {
        None
    }
    pub fn current_thread_id() -> Option<u64> {
        None
    }
    pub fn yield_current() {}
    pub struct Scheduler;
    impl Scheduler {
        pub fn block_current_for_timer(&mut self, _: u64) {}
    }
    pub fn with_current(f: impl FnOnce(&mut Scheduler)) {
        f(&mut Scheduler);
    }
}
pub mod kthread {
    pub fn kthread_run(_: fn(), _: &str) -> Result<(), ()> {
        Ok(())
    }
}
#[path = "../kernel/src/task/dispatch_strand_census.rs"]
mod census;

#[test]
fn census_preserves_lost_timer_wakes_and_legitimate_external_waits() {
    use crate::{scheduler::SAMPLE, thread::ThreadState::*};
    census::note_save(1);
    let cases = [
        (Some((BlockedOnTimer, None)), false),
        (Some((BlockedOnTimer, Some(1_500_000_000))), false),
        (Some((BlockedOnTimer, Some(2_000_000_000))), false),
        (Some((BlockedOnTimer, Some(2_000_000_001))), true),
        (Some((BlockedOnChildExit, None)), true),
        (Some((BlockedOnIO, None)), true),
        (Some((BlockedOnSignal, Some(1_500_000_000))), false),
        (Some((Ready, None)), false),
        (Some((Running, None)), false),
        (Some((Terminated, None)), false),
        (None, false), // missing row or busy owner cannot hide a saved thread
    ];
    for (sample, waiting) in cases {
        *SAMPLE.lock().unwrap() = sample;
        serial::OUTPUT.lock().unwrap().clear();
        census::report_snapshot();
        let output = serial::OUTPUT.lock().unwrap();
        let expected = if waiting {
            ":stranded=0:tids=-:"
        } else {
            ":stranded=1:tids=1:"
        };
        assert!(output.contains(expected), "{output}");
        assert!(
            output.contains(if waiting {
                ":waiting=1]"
            } else {
                ":waiting=0]"
            }),
            "{output}"
        );
    }
}
