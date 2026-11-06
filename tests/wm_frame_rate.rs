#[path = "../userspace/programs/src/wm_suite/rate.rs"]
mod rate;

fn stream(hz_microhz: u64, phase: u64, missed: bool) -> Vec<u64> {
    (0..310)
        .filter(|&n| !missed || n != 150)
        .map(|n| phase + n * 1_000_000_000_000_000 / hz_microhz)
        .collect()
}

#[test]
fn presentation_rate_is_independent_of_sample_phase_and_poll_delay() {
    for phase in 0..100 {
        let times = stream(60_019_800, phase * 166_000, false);
        let slow = stream(59_940_000, phase * 166_000, false);
        let missed = stream(60_019_800, phase * 166_000, true);
        for delay in [0, 100_000, 1_000_000, 5_000_000] {
            let begin = 20_000_000 + delay;
            let end = begin + 5_000_000_000;
            let sample = |v: &[u64]| v.iter().copied().filter(|&t| t >= begin && t < end).collect::<Vec<_>>();
            assert!(rate::measure(&sample(&times)).unwrap().passes);
            assert!(!rate::measure(&sample(&slow)).unwrap().passes);
            assert!(!rate::measure(&sample(&missed)).unwrap().passes);
        }
    }
}

#[test]
fn fewer_than_two_receipts_cannot_pass() {
    assert!(rate::measure(&[]).is_none());
    assert!(rate::measure(&[42]).is_none());
}
