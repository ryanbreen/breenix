#[path = "../userspace/programs/src/rtc_counter.rs"]
mod rtc_counter;

use rtc_counter::{next_edge, RtcPoll};

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
    // Return-to-return timing would accept 500 us and put this edge 12 ms late.
    assert!(new.after - old.after < 1_000_000 / 500);
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
    assert!(next_edge(first, || Ok(first), || true).unwrap_err().contains("1500 ms"));
    assert_eq!(next_edge(first, || Err("device error".into()), || false).unwrap_err(), "device error");
}
