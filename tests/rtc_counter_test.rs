#[path = "../userspace/programs/src/rtc_counter.rs"]
mod rtc_counter;

use rtc_counter::{drift_interval, frequency, next_edge, ppm_interval, select_edges, span_edges, RtcEdge, RtcPoll};

#[test]
fn delayed_old_second_cannot_masquerade_as_a_narrow_edge() {
    // The device advances at 1 s. An old response sampled before that edge
    // returns 12 ms late; the next read returns the new second in 500 us.
    let old = RtcPoll { second: 10, before: 999_500, after: 1_012_000 };
    let new = RtcPoll { second: 11, before: 1_012_000, after: 1_012_500 };
    let edge = next_edge(old, || Ok(new), || false).unwrap();
    assert_eq!(edge.second, 11);
    assert_eq!(edge.width, 13_000);
    assert!(edge.at - edge.width / 2 <= 1_000_000);
    assert!(edge.at + edge.width / 2 >= 1_000_000);
    assert!(edge.width > 1_000_000 / 500);
    assert_eq!(edge.at, 1_006_000);
}

#[test]
fn bounds_include_stalls_in_either_read_and_between_reads() {
    for old_after in [999_800, 1_012_000] {
        for new_before in [old_after, old_after + 12_000] {
            let old = RtcPoll { second: 59, before: 999_500, after: old_after };
            let new = RtcPoll { second: 0, before: new_before, after: new_before.max(1_000_000) + 500 };
            let edge = next_edge(old, || Ok(new), || false).unwrap();
            assert!(edge.at - edge.width / 2 <= 1_000_000);
            assert!(edge.at + (edge.width + 1) / 2 >= 1_000_000);
        }
    }
}

#[test]
fn uses_the_last_old_read_and_preserves_timeout_and_read_errors() {
    let first = RtcPoll { second: 10, before: 0, after: 100 };
    let last = RtcPoll { second: 10, before: 999_000, after: 999_500 };
    let new = RtcPoll { second: 11, before: 999_500, after: 1_000_500 };
    let mut reads = [last, new].into_iter();
    let edge = next_edge(first, || Ok(reads.next().unwrap()), || false).unwrap();
    assert_eq!(edge.width, 1500);
    assert_eq!(edge.at, 999_750);
    assert!(next_edge(first, || Ok(last), || true).unwrap_err().contains("1500 ms"));
    assert_eq!(next_edge(first, || Err("device error".into()), || false).unwrap_err(), "device error");
}

#[test]
fn rejects_backward_counters_within_and_between_reads() {
    let old = RtcPoll { second: 10, before: 1000, after: 1500 };
    for new in [
        RtcPoll { second: 11, before: 500, after: 900 },
        RtcPoll { second: 11, before: 1500, after: 1400 },
        RtcPoll { second: 10, before: 500, after: 900 },
    ] {
        assert!(next_edge(old, || Ok(new), || false).unwrap_err().contains("backwards"));
    }
    let reversed = RtcPoll { second: 10, before: 1500, after: 1000 };
    assert!(next_edge(reversed, || Ok(old), || false).unwrap_err().contains("backwards"));
}

#[test]
fn slow_reads_widen_frequency_bounds_without_rejecting_the_measurement() {
    let narrow = frequency(
        RtcEdge { second: 10, at: 1_000_000, width: 500 },
        RtcEdge { second: 14, at: 5_000_000, width: 500 },
    ).unwrap();
    let wide = frequency(
        RtcEdge { second: 10, at: 1_000_000, width: 7000 },
        RtcEdge { second: 14, at: 5_000_000, width: 9000 },
    ).unwrap();
    assert_eq!(narrow, (1_000_000, 999_875, 1_000_125));
    assert_eq!(wide, (1_000_000, 998_000, 1_002_000));
}

#[test]
fn bounds_enclose_the_rate_when_old_read_returns_after_the_edge() {
    let first = next_edge(
        RtcPoll { second: 10, before: 999_500, after: 1_012_000 },
        || Ok(RtcPoll { second: 11, before: 1_012_000, after: 1_012_500 }),
        || false,
    ).unwrap();
    let last = RtcEdge { second: 15, at: 5_000_000, width: 1000 };
    let (midpoint, low, high) = frequency(first, last).unwrap();
    assert!(midpoint < 1_000_000);
    assert!(low <= 1_000_000 && high >= 1_000_000);
}

#[test]
fn odd_widths_and_fractional_hertz_round_bounds_outward() {
    let result = frequency(
        RtcEdge { second: 86_398, at: 10, width: 3 },
        RtcEdge { second: 2, at: 51, width: 5 },
    ).unwrap();
    assert_eq!(result, (10, 9, 12));
}

#[test]
fn invalid_or_overlapping_edge_brackets_do_not_measure_a_frequency() {
    let first = RtcEdge { second: 10, at: 1000, width: 100 };
    for last in [
        RtcEdge { second: 10, at: 2000, width: 100 },
        RtcEdge { second: 14, at: 900, width: 100 },
        RtcEdge { second: 14, at: 1000, width: 100 },
        RtcEdge { second: 14, at: 1050, width: 100 },
        RtcEdge { second: 14, at: 1100, width: 100 },
        RtcEdge { second: 14, at: 2000, width: 6000 },
        RtcEdge { second: 14, at: u64::MAX, width: 100 },
    ] {
        assert!(frequency(first, last).is_err(), "{last:?}");
    }
}

fn edge(second: i64, width: u64) -> RtcEdge {
    RtcEdge { second: second.rem_euclid(86_400), at: second as u64 * 1_000_000, width }
}

#[test]
fn selection_keeps_narrow_start_and_waits_four_seconds() {
    let mut edges = (10..=14).map(|second| Ok(edge(second, 500)));
    let (first, last, passed) = select_edges(|| edges.next().unwrap()).unwrap();
    assert_eq!((first.second, last.second, passed), (10, 14, 0));
    assert!(edges.next().is_none());
}

#[test]
fn selection_replaces_a_wide_start_with_a_narrow_edge() {
    let mut edges = [edge(10, 7000), edge(11, 500), edge(15, 500)].into_iter();
    let (first, last, passed) = select_edges(|| Ok(edges.next().unwrap())).unwrap();
    assert_eq!((first.second, last.second, passed), (11, 15, 1));
}

#[test]
fn selection_passes_over_wide_end_without_discarding_narrow_start() {
    let mut edges = [edge(10, 500), edge(14, 7000), edge(15, 500)].into_iter();
    let (first, last, passed) = select_edges(|| Ok(edges.next().unwrap())).unwrap();
    assert_eq!((first.second, last.second, passed), (10, 15, 1));
}

#[test]
fn selection_stops_at_two_passovers_and_extends_slow_read_span() {
    let mut edges = (10..=26).map(|second| Ok(edge(second, 7000)));
    let (first, last, passed) = select_edges(|| edges.next().unwrap()).unwrap();
    assert_eq!((first.second, last.second, passed), (12, 26, 2));
    assert_eq!(frequency(first, last).unwrap(), (1_000_000, 999_500, 1_000_500));
}

#[test]
fn selection_returns_unresolved_bounds_at_sixteen_seconds_across_midnight() {
    let mut edges = (86_398..=86_416).map(|second| Ok(edge(second, 20_000)));
    let (first, last, passed) = select_edges(|| edges.next().unwrap()).unwrap();
    assert_eq!((first.second, last.second, passed), (0, 16, 2));
    assert_eq!(frequency(first, last).unwrap(), (1_000_000, 998_750, 1_001_250));
}

#[test]
fn selection_preserves_read_and_invalid_counter_errors() {
    assert_eq!(select_edges(|| Err("RTC error".into())).unwrap_err(), "RTC error");
    let first = edge(10, 500);
    for last in [edge(10, 500), RtcEdge { at: first.at - 1, ..edge(11, 500) },
                 RtcEdge { at: first.at, ..edge(11, 500) }] {
        let mut edges = [first, last].into_iter();
        assert_eq!(select_edges(|| Ok(edges.next().unwrap())).unwrap_err(), frequency(first, last).unwrap_err());
    }
}

#[test]
fn uncertain_midpoint_cannot_pass_the_drift_check() {
    let (lower, upper) = drift_interval(4_000_000_000, 4_000_000, 998_258, 1_001_742).unwrap();
    assert_eq!((lower, upper), (-1742, 1742));
    assert!(!(lower >= -1000 && upper <= 1000));
    let (lower, upper) = drift_interval(4_000_000_000, 4_000_000, 999_500, 1_000_500).unwrap();
    assert_eq!((lower, upper), (-500, 500));
    assert!(lower >= -1000 && upper <= 1000);
}

#[test]
fn drift_bounds_round_outward_at_both_tolerance_edges() {
    assert_eq!(drift_interval(999_000_000, 1_000_000, 1_000_000, 1_000_000).unwrap(), (-1000, -1000));
    assert_eq!(drift_interval(998_999_999, 1_000_000, 1_000_000, 1_000_000).unwrap(), (-1001, -1000));
    assert_eq!(drift_interval(1_001_000_001, 1_000_000, 1_000_000, 1_000_000).unwrap(), (1000, 1001));
    for (elapsed, cycles, lower, upper) in [(0, 1, 1, 1), (1, 0, 1, 1), (1, 1, 0, 1), (1, 1, 2, 1)] {
        assert!(drift_interval(elapsed, cycles, lower, upper).is_err());
    }
}

/// span_edges' source: the queued edges in order, recording when each was expected.
fn queued(edges: Vec<RtcEdge>) -> (impl FnMut(Option<u64>) -> Result<RtcEdge, String>, std::rc::Rc<std::cell::RefCell<Vec<Option<u64>>>>) {
    let asked = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let log = asked.clone();
    let mut edges = edges.into_iter();
    (move |expected| { log.borrow_mut().push(expected); edges.next().ok_or_else(|| "no more edges".to_string()) }, asked)
}

#[test]
fn span_measures_from_a_narrow_edge_to_the_narrow_edge_three_seconds_later() {
    let (source, asked) = queued(vec![edge(10, 500), edge(13, 500)]);
    let (first, last, passed) = span_edges(3, 1_000_000, 1000, 2, source).unwrap();
    assert_eq!((first.second, last.second, passed), (10, 13, 0));
    assert_eq!(*asked.borrow(), vec![None, Some(13_000_000)]);
}

#[test]
fn span_passes_over_wide_edges_at_either_end_and_counts_them() {
    let (source, asked) = queued(vec![edge(10, 7000), edge(11, 500), edge(14, 7000), edge(15, 500)]);
    let (first, last, passed) = span_edges(3, 1_000_000, 1000, 2, source).unwrap();
    assert_eq!((first.second, last.second, passed), (11, 15, 2));
    assert_eq!(*asked.borrow(), vec![None, Some(11_000_000), Some(14_000_000), Some(15_000_000)]);
}

#[test]
fn span_refuses_to_measure_once_too_many_edges_are_wide() {
    let (source, _) = queued(vec![edge(10, 7000), edge(11, 500), edge(14, 7000), edge(15, 7000)]);
    let error = span_edges(3, 1_000_000, 1000, 2, source).unwrap_err();
    assert!(error.starts_with("3 RTC seconds edges were bracketed by reads wider than 1000 counts (the last 7000)"), "{error}");
    let (source, _) = queued(vec![edge(10, 7000), edge(11, 7000), edge(12, 7000)]);
    assert!(span_edges(3, 1_000_000, 1000, 2, source).is_err());
}

#[test]
fn span_accepts_a_longer_span_when_an_expected_edge_was_missed() {
    // Woken after the edge expected at 13 s, the source returns the next one.
    let (source, _) = queued(vec![edge(86_399, 500), edge(86_403, 500)]);
    let (first, last, passed) = span_edges(3, 1_000_000, 1000, 2, source).unwrap();
    assert_eq!((first.second, last.second, passed), (86_399, 3, 0));
    assert_eq!(frequency(first, last).unwrap().0, 1_000_000);
}

#[test]
fn span_keeps_reading_past_an_edge_too_early_for_the_span() {
    let (source, _) = queued(vec![edge(10, 500), edge(12, 500), edge(13, 500)]);
    let (first, last, passed) = span_edges(3, 1_000_000, 1000, 2, source).unwrap();
    assert_eq!((first.second, last.second, passed), (10, 13, 0));
    let (source, _) = queued(vec![]);
    assert_eq!(span_edges(3, 1_000_000, 1000, 2, source).unwrap_err(), "no more edges");
}

#[test]
fn ppm_interval_rounds_outward_at_both_tolerance_edges() {
    assert_eq!(ppm_interval(998_000_000, 1_002_000_000, 1_000_000_000), (-2000, 2000));
    assert_eq!(ppm_interval(997_999_999, 1_002_000_001, 1_000_000_000), (-2001, 2001));
    assert_eq!(ppm_interval(1_000_000_000, 1_000_000_000, 1_000_000_000), (0, 0));
    assert_eq!(ppm_interval(999_999_999, 1_000_000_001, 1_000_000_000), (-1, 1));
}
