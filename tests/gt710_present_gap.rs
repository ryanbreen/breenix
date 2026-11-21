#[path = "../userspace/programs/src/suite_breengel/window_cases/present_gap.rs"]
mod present_gap;

#[test]
fn selects_the_longest_measured_composite_gap() {
    let window = 7;
    let mut rows = Vec::new();
    // The measured interval begins after a longer startup gap and ends before
    // a longer teardown gap. The missed edge falls outside a 128-row tail.
    let cycle = |i: u64| match i {
        0 => 100,
        162 => 1_000,
        _ => 200 + i + u64::from(i >= 12),
    };
    let receipt = |id, serial, commit, at| [[12, serial, 900, 0, at], [13, id, commit, serial, at + 4]];
    for i in 0..=162 {
        rows.extend(receipt(window, i + 1, 2 * i, 10 * cycle(i)));
    }
    let before = 10 * cycle(11);
    let after = 10 * cycle(12);
    // A hardware timestamp alone does not indicate a counted composite.
    // Another window's entry in the same composite must not count twice.
    rows.push([12, 999, 900, 0, before + 5]);
    rows.push([13, window + 1, 2, 13, after + 4]);
    rows.extend([
        [14, window, 22, 0, before - 7],
        [14, window, 23, 0, before + 2],
        [14, window, 24, 0, before + 5],
        [49, 1, 0, 0, before + 5],
        [47, 13, before + 6, before + 7, before + 8],
        [10, 13, 900, 0, before + 10],
        [50, 900, after, after, before + 11],
        [48, 900, after, after + 1, after + 2],
        [11, 13, 900, 200, after + 3],
        // Same target, different native latch. It cannot fill the chosen row.
        [48, 900, after + 20, after + 21, after + 22],
    ]);
    let start = 10 * cycle(1);
    let end = 10 * cycle(161);
    let report = present_gap::lines(&rows, window, start, end, 160).unwrap();
    assert_eq!(report.len(), 3);
    assert!(report[0].contains(&format!("gap_ns=20 before_ns={before} after_ns={after}")));
    assert!(report[1].contains("serial=12 commit=22"));
    assert!(report[2].contains("serial=13 commit=24"));
    for (name, stamp) in [
        ("first_ready_ns", before + 2), ("front_ready_ns", before + 5),
        ("scene_read_ns", before + 5), ("compose_begin_ns", before + 6),
        ("submit_begin_ns", before + 7), ("submit_return_ns", before + 8),
        ("render_ready_ns", before + 10), ("native_accept_ns", before + 11),
        ("predicted_latch_ns", after), ("presented_ns", after),
        ("native_complete_ns", after + 1), ("native_observed_ns", after + 2),
        ("present_return_ns", after + 3), ("receipt_ns", after + 4),
    ] {
        assert!(report[2].contains(&format!(" {name}={stamp}")), "{}", report[2]);
    }
    // A missing initial boundary or an incomplete retained interval cannot
    // support a claim that any surviving pair was the longest gap.
    assert!(present_gap::lines(&rows[4..], window, start, end, 160).is_none());
    rows.retain(|r| !(r[0] == 13 && r[3] == 100));
    assert!(present_gap::lines(&rows, window, start, end, 160).is_none());
}
