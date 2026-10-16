//! Behavioral tests of the kernel's atomic POSIX timer notification state.
mod signal {
    mod timer_signal {
        include!(concat!(env!("CARGO_MANIFEST_DIR"), "/kernel/src/signal/timer_signal.rs"));
    }
    #[cfg(test)]
    mod tests {
        use super::{timer_signal::{TimerSignal, REBIND, JOB}};
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
            assert_eq!(timer.due_order().1, core::cmp::Reverse(0));
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
        fn rearming_resets_previous_arming_counts() {
            let timer = TimerSignal::new(1);
            assert!(timer.expire(4));
            assert_eq!(timer.overrun(), 3);
            timer.reset_overruns();
            assert_eq!(timer.last_overrun.load(Ordering::Acquire), 0);
            assert_eq!(timer.overrun(), 0);
            assert_eq!(timer.deliver(), 0);
            assert!(timer.expire(1));
            assert_eq!(timer.overrun(), 0);
        }

        #[test]
        fn queued_then_rebound_notification_stays_active() {
            let timer = TimerSignal::new(1);
            assert!(timer.expire(1));
            assert!(timer.is_pending());
            assert_eq!(timer.next_notification_check(100, 5), Some(105));
            assert_eq!(timer.overrun(), 0);
            assert!(!timer.expire(3));
            assert_eq!(timer.deliver(), 0);
            assert!(timer.is_pending());
            assert_eq!(timer.next_notification_check(100, 5), Some(100));
            timer.clear_due();
            assert!(timer.is_pending());
            assert_eq!(timer.overrun(), 2);
            assert_eq!(timer.deliver(), 2);
            assert!(!timer.is_pending());
            assert_eq!(timer.next_notification_check(100, 5), None);
        }

        #[test]
        fn job_control_rebound_uses_softirq_once() {
            let timer = TimerSignal::new(1);
            assert!(timer.expire(1));
            assert_eq!(timer.overrun(), 0);
            assert!(!timer.expire(1));
            assert_eq!(timer.deliver(), 0);
            assert!(!timer.rebind_job(false));
            assert_eq!(timer.due_for.load(Ordering::Acquire), REBIND);
            assert!(timer.rebind_job(true));
            assert_eq!(timer.due_for.load(Ordering::Acquire), JOB);
            assert!(!timer.rebind_job(true));
            timer.clear_due();
            assert_eq!(timer.deliver(), 0);
            assert!(timer.expire(1));
        }

        #[test]
        fn pending_overrun_is_what_delivery_reports_and_records_nothing() {
            let timer = TimerSignal::new(1);
            assert!(timer.expire(3));
            assert_eq!(timer.pending_overrun(), 2);
            // A claim that is dropped undelivered leaves no snapshot behind:
            // expiries counted after it still join this notification.
            assert!(!timer.expire(2));
            assert_eq!(timer.pending_overrun(), 4);
            assert_eq!(timer.deliver(), 4);
            assert!(!timer.is_pending());
            assert!(timer.expire(1));
            assert_eq!(timer.overrun(), 0);
            assert!(!timer.expire(2));
            assert_eq!(timer.pending_overrun(), 0);
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
