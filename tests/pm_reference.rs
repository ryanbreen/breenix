#[path = "../kernel/src/pm_reference.rs"]
mod pm_reference;

use pm_reference::{measure, Sample, PM_HZ};

const MASK: u32 = 0x00ff_ffff;

fn reading(cycles: u64, width: u64) -> Sample {
    // Begin near wrap to exercise a legitimate 24-bit counter rollover.
    let ticks = (u64::from(MASK) - PM_HZ / 2 + cycles * PM_HZ / 1_000_000_000)
        as u32 & MASK;
    Sample { ticks, before: cycles, after: cycles + width }
}

#[test]
fn measures_a_full_second_across_counter_wrap() {
    let mut cycles = 0;
    let hz = measure(|| {
        let s = reading(cycles, 100);
        cycles += 10_000_000;
        s
    }, MASK).unwrap();
    assert!(hz.abs_diff(1_000_000_000) <= 1000);
}

#[test]
fn retakes_a_sample_with_a_two_ms_endpoint_bracket() {
    let mut cycles = 0;
    let mut delayed = false;
    let hz = measure(|| {
        let width = if cycles == 1_000_000_000 {
            delayed = true;
            2_000_000
        } else { 100 };
        let s = reading(cycles, width);
        cycles += 10_000_000;
        s
    }, MASK).unwrap();
    assert!(delayed);
    assert!(cycles > 2_000_000_000, "the interrupted interval must be discarded");
    assert!(hz.abs_diff(1_000_000_000) <= 1000);
}

#[test]
fn retakes_after_a_gap_longer_than_a_pm_wrap() {
    let mut cycles = 0;
    let mut stalled = false;
    let hz = measure(|| {
        if cycles == 500_000_000 {
            cycles += 5_000_000_000;
            stalled = true;
        }
        let s = reading(cycles, 100);
        cycles += 10_000_000;
        s
    }, MASK).unwrap();
    assert!(stalled);
    assert!(cycles > 6_500_000_000);
    assert!(hz.abs_diff(1_000_000_000) <= 1000);
}

#[test]
fn refuses_repeated_stalls_and_a_dead_timer() {
    let mut cycles = 0;
    assert!(measure(|| {
        let s = reading(cycles, 2_000_000);
        cycles += 10_000_000;
        s
    }, MASK).is_none());
    let mut cycles = 0;
    assert!(measure(|| {
        let s = Sample { ticks: 7, before: cycles, after: cycles + 100 };
        cycles += 10_000_000;
        s
    }, MASK).is_none());
}
