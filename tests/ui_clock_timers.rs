#[path = "../libs/breengel/src/timer.rs"]
mod timer;
use timer::Timers;

#[test]
fn equal_deadlines_follow_registration_order() {
    let mut timers = Timers::new(0);
    timers.register(2, 10, 0);
    timers.register(1, 10, 0);
    assert_eq!(timers.step(10), vec![(2, 0), (1, 0)]);
}

#[test]
fn missed_periods_fire_once() {
    let mut timers = Timers::new(0);
    timers.register(1, 100, 100);
    assert_eq!(timers.step(1000), vec![(1, 9)]);
}

#[test]
fn zero_step_fires_only_due_timers() {
    let mut timers = Timers::new(10);
    timers.register(1, 10, 0);
    timers.register(2, 11, 0);
    assert_eq!(timers.step(0), vec![(1, 0)]);
    assert!(timers.step(0).is_empty());
}

#[test]
fn large_step_integrates_each_millisecond() {
    fn advance(parts: &[u64]) -> (u64, u64) {
        let mut timers = Timers::new(0);
        timers.register(1, 1, 1);
        let mut animation = 0;
        for &part in parts {
            for (_, missed) in timers.step(part) { animation += missed + 1; }
        }
        (animation, timers.now())
    }
    assert_eq!(advance(&[1000]), advance(&[1; 1000]));
    assert_eq!(advance(&[1000]), (1000, 1000));
}
