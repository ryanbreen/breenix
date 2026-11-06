#[path = "../kernel/src/time/deadlines.rs"]
pub mod deadlines;
mod time { pub use crate::deadlines; }
// Clock-event scheduling preserves regular ticks while inserting fine wakes.
#[path = "../kernel/src/arch_impl/x86_64/apic/clockevent.rs"]
mod clockevent;
use clockevent::ClockEvent;
const MS: u64 = 1_000_000;
#[test]
fn fine_wake_preserves_the_existing_tick_and_restores_periodic_mode() {
    let mut event = ClockEvent::new();
    assert_eq!(event.request(10 * MS, 3 * MS, 10 * MS + 250_000).unwrap(), 10 * MS + 250_000);
    assert_eq!(event.wake_deadline(), 10 * MS + 250_000);
    assert_eq!(event.interrupt(10 * MS + 250_000, 5 * MS), (false, true, Some(13 * MS)));
    assert_eq!(event.interrupt(13 * MS, 5 * MS), (true, false, None));
    assert_eq!(event.request(20 * MS, 4 * MS, 20 * MS + 100_000).unwrap(), 20 * MS + 100_000);
}
#[test]
fn overlapping_requests_expire_without_waiter_cleanup() {
    let mut event = ClockEvent::new();
    event.request(0, 5 * MS, 250_000).unwrap();
    event.request(100_000, 4 * MS, 350_000).unwrap();
    assert_eq!(event.interrupt(250_000, 5 * MS), (false, true, Some(350_000)));
    assert_eq!(event.interrupt(350_000, 5 * MS), (false, true, Some(5 * MS)));
    assert_eq!(event.interrupt(5 * MS, 5 * MS), (true, false, None));
}
#[test]
fn earlier_requests_win_and_late_interrupts_do_not_repeat_old_ticks() {
    let mut event = ClockEvent::new();
    event.request(0, 5 * MS, MS).unwrap();
    assert_eq!(event.request(10, 999, 500_000).unwrap(), 500_000);
    assert_eq!(event.interrupt(400_000, 5 * MS), (false, false, Some(500_000)));
    assert_eq!(event.interrupt(16 * MS, 5 * MS), (true, true, None));
}
#[test]
fn regular_ticks_continue_during_a_long_fine_deadline() {
    let mut event = ClockEvent::new();
    assert_eq!(event.request(0, 2 * MS, 20 * MS).unwrap(), 2 * MS);
    assert_eq!(event.interrupt(2 * MS, 5 * MS), (true, false, Some(7 * MS)));
    assert_eq!(event.interrupt(7 * MS, 5 * MS), (true, false, Some(12 * MS)));
    assert_eq!(event.interrupt(12 * MS, 5 * MS), (true, false, Some(17 * MS)));
    assert_eq!(event.interrupt(17 * MS, 5 * MS), (true, false, Some(20 * MS)));
    assert_eq!(event.interrupt(20 * MS, 5 * MS), (false, true, Some(22 * MS)));
    assert_eq!(event.interrupt(22 * MS, 5 * MS), (true, false, None));
}

#[test]
fn distant_deadline_does_not_create_unrequested_poll_interrupts() {
    let mut event = ClockEvent::new();
    event.request(0, 5 * MS, 250_000).unwrap();
    event.request(0, 5 * MS, 1_000 * MS).unwrap();
    assert_eq!(event.interrupt(250_000, 5 * MS),
               (false, true, Some(5 * MS)));
    let mut wakes = 1;
    for now in (5..=1000).step_by(5) {
        let (_, wake, _) = event.interrupt(now * MS, 5 * MS);
        wakes += usize::from(wake);
    }
    assert_eq!(wakes, 2);
}

#[test]
fn capacity_refuses_new_deadlines_without_losing_accepted_ones() {
    let mut deadlines = deadlines::Deadlines::new();
    for n in (1..=64).rev() { assert!(deadlines.insert(n)); }
    assert!(deadlines.insert(32));
    assert!(!deadlines.insert(65));
    for n in 1..=64 { assert_eq!(deadlines.next(), n); deadlines.expire(n); }
    assert_eq!(deadlines.next(), u64::MAX);
}
