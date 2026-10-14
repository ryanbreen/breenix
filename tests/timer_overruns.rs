//! Behavioral tests of the kernel's atomic POSIX timer notification state.
mod signal {
    mod timers {
        use core::sync::atomic::{AtomicU64, AtomicUsize};
        pub(super) const QUEUED: u64 = 1 << 63;
        pub(super) const REBIND: usize = usize::MAX;
        pub(super) const JOB: usize = usize::MAX - 1;
        pub(super) const NOT_SHOWN: u64 = u64::MAX;
        pub(super) static NEXT_DUE: AtomicU64 = AtomicU64::new(1);
        pub(super) static JOB_DUE: AtomicUsize = AtomicUsize::new(0);
    }
    mod timer_signal {
        include!(concat!(env!("CARGO_MANIFEST_DIR"), "/kernel/src/signal/timer_signal.rs"));
    }
    #[cfg(test)]
    mod tests {
        use super::{timer_signal::TimerSignal, timers::REBIND};
        use core::sync::atomic::Ordering;

        #[test]
        fn delayed_discovery_coalesces_four_expirations() {
            let timer = TimerSignal::new(7);
            assert_eq!(timer.id, 7);
            assert!(timer.expire(4));
            assert_eq!(timer.overrun(), 3);
            assert_eq!(timer.deliver(), 3);
            assert_eq!(timer.last_overrun.load(Ordering::Acquire), 3);
            assert!(timer.expire(1));
            assert_eq!(timer.deliver(), 0);
            let _ = timer.due_order();
        }

        #[test]
        fn expirations_after_siginfo_snapshot_generate_another_notification() {
            let timer = TimerSignal::new(1);
            assert!(timer.expire(1));
            assert_eq!(timer.overrun(), 0);
            assert!(!timer.expire(3));
            assert_eq!(timer.deliver(), 0);
            assert_eq!(timer.due_for.load(Ordering::Acquire), REBIND);
            assert_eq!(timer.overrun(), 2);
            assert_eq!(timer.deliver(), 2);
            assert_eq!(timer.last_overrun.load(Ordering::Acquire), 2);
            assert!(timer.expire(1));
            assert_eq!(timer.deliver(), 0);
        }

        #[test]
        fn another_expiry_can_join_the_rebound_notification() {
            let timer = TimerSignal::new(1);
            assert!(timer.expire(2));
            assert_eq!(timer.overrun(), 1);
            assert!(!timer.expire(2));
            assert_eq!(timer.deliver(), 1);
            assert!(!timer.expire(1));
            assert_eq!(timer.deliver(), 2);
        }

        #[test]
        fn discard_resets_snapshot_and_counts() {
            let timer = TimerSignal::new(1);
            assert!(timer.expire(4));
            assert_eq!(timer.overrun(), 3);
            timer.discard();
            assert!(timer.expire(1));
            assert_eq!(timer.deliver(), 0);
        }

        #[test]
        fn overrun_saturates_at_the_abi_maximum() {
            let timer = TimerSignal::new(1);
            assert!(timer.expire(u64::MAX));
            assert_eq!(timer.overrun(), i32::MAX);
            assert_eq!(timer.deliver(), i32::MAX);
        }
    }
}
