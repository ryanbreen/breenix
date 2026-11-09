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

#[test]
fn nonlinear_animation_runs_each_quantum_before_coalesced_callbacks() {
    fn run(parts: &[u64]) -> (i64, i64, Vec<(u64, u64)>, u64) {
        let mut timers = Timers::new(0);
        timers.register(1, 1000, 1000);
        let (mut position, mut velocity) = (0_i64, 0_i64);
        let mut ticks = Vec::new();
        let mut fired = Vec::new();
        for &part in parts {
            fired.extend(timers.advance(part, |time_ms| {
                ticks.push(time_ms);
                velocity += (1000 - position) / 16;
                position += velocity;
                velocity = velocity * 31 / 32;
            }));
        }
        assert_eq!(ticks, (1..=1000).collect::<Vec<_>>());
        (position, velocity, fired, timers.next_deadline())
    }
    let large = run(&[1000]);
    assert_eq!(large, run(&[1; 1000]));
    assert_eq!(large.2, vec![(1, 0)]);
    assert_eq!(large.3, 2000);
    assert_ne!((large.0, large.1), (62, 60)); // One oversized integration update.
}

#[test]
fn deadlines_sort_before_registration_and_zero_has_no_integration() {
    let mut timers = Timers::new(10);
    timers.register(1, 12, 0);
    timers.register(2, 10, 0);
    assert_eq!(timers.advance(0, |_| panic!("zero step integrated")), vec![(2, 0)]);
    assert_eq!(timers.next_deadline(), 12);
    assert_eq!(timers.step(2), vec![(1, 0)]);
    assert_eq!(timers.next_deadline(), u64::MAX);
}
