extern crate alloc;
#[path = "../kernel/src/console/status.rs"]
pub mod status;

use status::{PanelGeometry, PanelMode, Rect, SuiteStatus};

#[test]
fn serial_results_drive_counts_current_case_and_elapsed() {
    let mut s = SuiteStatus::default();
    assert_eq!(s.mode(false), PanelMode::Stages);
    assert_eq!(s.mode(true), PanelMode::Empty);
    assert_eq!(s.heading(500), None);
    assert!(s.observe(b"SUITE console START cases=3", 1_000));
    assert_eq!(
        s.heading(62_000).unwrap(),
        "console: 0 passed · 0 failed · 3 pending of 3 · 1:01 · case: waiting"
    );
    assert!(s.observe(b"SUITE console CASE text/write PASS ms=12", 63_000));
    assert!(s.observe(
        b"SUITE console CASE text/scroll FAIL ms=9 msg=wrong pixel",
        64_000
    ));
    assert_eq!(
        (s.passed(), s.failed(), s.pending(), s.total()),
        (1, 1, 1, 3)
    );
    assert_eq!(s.current_case(), Some("text/scroll"));
    assert_eq!(s.elapsed(65_000), (1, 4));
    assert_eq!(s.mode(true), PanelMode::Suite);
    assert!(s.observe(b"SUITE console CASE text/colour PASS ms=0", 66_000));
    assert!(s.observe(
        b"SUITE console DONE passed=2 failed=1 skipped=0 total=3",
        67_000
    ));
    assert_eq!(s.elapsed(1_000_000), (1, 6));
    assert!(!s.observe(b"SUITE console CASE text/late PASS ms=1", 68_000));
}

#[test]
fn malformed_foreign_duplicate_and_inconsistent_records_are_ignored() {
    let mut s = SuiteStatus::default();
    assert!(!s.observe(b"SUITE console CASE text/early PASS ms=1", 0));
    assert!(s.observe(b"SUITE console START cases=2", 100));
    let baseline = s.clone();
    for line in [
        &b"will emit SUITE console START cases=99"[..],
        b"SUITE console START cases=-1",
        b"SUITE console START cases=184467440737095516160",
        b"SUITE console START cases=2 cases=3",
        b"SUITE console START cases=2 garbage",
        b"SUITE other CASE text/write PASS ms=1",
        b"SUITE console CASE no-category PASS ms=1",
        b"SUITE console CASE text/write PASS",
        b"SUITE console CASE text/write PASS ms=x",
        b"SUITE console CASE text/write PASS ms=1 garbage",
        b"SUITE console CASE text/write FAIL ms=1",
        b"SUITE console CASE text/write FAIL ms=1 msg=",
        b"SUITE console CASE text/write SKIP msg=",
        b"SUITE console CASE text/write PASS ms=1 sha256=bad",
        b"SUITE console CASE text/\xff PASS ms=1",
        b"SUITE console DONE passed=2 failed=0 skipped=0 total=2",
    ] {
        assert!(!s.observe(line, 200), "accepted {:?}", line);
        assert_eq!(s, baseline, "mutated on {:?}", line);
    }
    assert!(s.observe(b"SUITE console CASE text/write PASS ms=1", 300));
    let once = s.clone();
    assert!(!s.observe(
        b"SUITE console CASE text/write FAIL ms=2 msg=duplicate",
        400
    ));
    assert_eq!(s, once);
    assert_eq!(s.elapsed(99), (0, 0));
}

#[test]
fn skips_extensions_crlf_and_second_suite_reset() {
    let mut s = SuiteStatus::default();
    assert!(s.observe(b"SUITE first START cases=3 only=text pace_ms=200\r\n", 10));
    let hash = "a".repeat(64);
    assert!(s.observe(
        format!("SUITE first CASE text/pass PASS ms=1 sha256={hash}").as_bytes(),
        20
    ));
    assert!(s.observe(b"SUITE first CASE text/skip SKIP msg=not applicable", 30));
    assert!(s.observe(
        format!("SUITE first CASE text/fail FAIL ms=2 sha256={hash} msg=bad").as_bytes(),
        40
    ));
    assert_eq!(
        (s.passed(), s.failed(), s.skipped(), s.pending()),
        (1, 1, 1, 0)
    );
    assert!(s.heading(50).unwrap().contains(" · 1 skipped"));
    assert!(s.observe(
        b"SUITE first DONE passed=1 failed=1 skipped=1 total=3 only=text",
        60
    ));
    assert!(s.observe(b"SUITE second START cases=0", 100_000));
    assert_eq!(
        (s.passed(), s.failed(), s.skipped(), s.pending(), s.total()),
        (0, 0, 0, 0, 0)
    );
    assert_eq!(s.current_case(), None);
    assert_eq!(s.elapsed(101_000), (0, 1));
    assert!(s.observe(
        b"SUITE second DONE passed=0 failed=0 skipped=0 total=0",
        102_000
    ));
    assert_eq!(s.elapsed(200_000), (0, 2));
}

#[test]
fn stage_geometry_is_exact_at_whole_scales() {
    for scale in [1, 2, 3] {
        let g = PanelGeometry::for_display(1280 * scale, 800 * scale, 9, 22).unwrap();
        assert_eq!(g.panel, Rect::new(0, 704 * scale, 1280 * scale, 96 * scale));
        assert_eq!(
            g.heading,
            Rect::new(16 * scale, 712 * scale, 1248 * scale, 24 * scale)
        );
        assert_eq!(
            g.track,
            Rect::new(16 * scale, 744 * scale, 1248 * scale, 12 * scale)
        );
        assert_eq!(
            g.label,
            Rect::new(16 * scale, 768 * scale, 80 * scale, 16 * scale)
        );
        assert_eq!(
            g.ribbon,
            Rect::new(112 * scale, 772 * scale, 1152 * scale, 6 * scale)
        );
        assert_eq!(g.stage_scale, scale);
        assert_eq!(g.text_rows(22), 704 * scale / 22);
    }
}

#[test]
fn native_raster_reserves_panel_above_scroll_region() {
    let g = PanelGeometry::for_display(1024, 768, 7, 18).unwrap();
    assert_eq!(g.stage_scale, 0);
    assert_eq!(g.panel, Rect::new(0, 684, 1024, 84));
    assert_eq!(g.heading, Rect::new(14, 692, 996, 18));
    assert_eq!(g.track, Rect::new(14, 718, 996, 8));
    assert_eq!(g.label, Rect::new(14, 734, 77, 18));
    assert_eq!(g.ribbon, Rect::new(98, 740, 912, 6));
    assert_eq!(g.text_rows(18), 38);
    let g = PanelGeometry::for_display(1280, 960, 9, 22).unwrap();
    assert_eq!(g.panel, Rect::new(0, 868, 1280, 92));
    assert_eq!(g.text_rows(22), 39);
    assert!(PanelGeometry::for_display(80, 60, 7, 18).is_none());
}

#[test]
fn progress_has_exact_fill_and_failure_positions() {
    let g = PanelGeometry::for_display(1280, 800, 9, 22).unwrap();
    let mut s = SuiteStatus::default();
    assert_eq!(g.progress_colour(&s, 16, 744), None);
    assert!(s.observe(b"SUITE console START cases=4", 0));
    assert!(s.observe(b"SUITE console CASE text/one PASS ms=1", 1));
    assert!(s.observe(b"SUITE console CASE text/skip SKIP msg=unsupported", 2));
    assert!(s.observe(b"SUITE console CASE text/two FAIL ms=1 msg=bad", 3));
    for x in 16..1264 {
        let expected = if x < 328 {
            [26, 112, 77]
        } else if x < 640 {
            [145, 50, 58]
        } else {
            [48, 67, 96]
        };
        assert_eq!(g.progress_colour(&s, x, 744), Some(expected), "x={x}");
    }
    assert_eq!(g.progress_colour(&s, 16, 756), None);
    assert_eq!(g.progress_colour(&s, 15, 744), None);
    assert!(s.observe(b"SUITE empty START cases=0", 4));
    assert_eq!(g.progress_colour(&s, 16, 744), Some([48, 67, 96]));
}

#[test]
fn ribbon_colours_gaps_and_bounds_follow_the_amended_band() {
    for (w, h, glyph, line) in [(1280, 800, 9, 22), (2560, 1600, 14, 34), (1024, 768, 7, 18)] {
        let g = PanelGeometry::for_display(w, h, glyph, line).unwrap();
        let r = g.ribbon;
        let gap = 2 * g.stage_scale.max(1);
        assert_eq!(
            g.stage_colour(&[true, false], r.x, r.y),
            Some([26, 112, 77])
        );
        assert_eq!(
            g.stage_colour(&[true, false], r.x + r.width / 2, r.y),
            Some([48, 67, 96])
        );
        for x in r.x + r.width / 2 - gap..r.x + r.width / 2 {
            assert_eq!(g.stage_colour(&[true, false], x, r.y), Some([12, 19, 31]));
        }
        assert_eq!(g.stage_colour(&[], r.x, r.y), None);
        assert_eq!(g.stage_colour(&[true], r.x, r.y - 1), None);
        assert_eq!(g.stage_colour(&[true], r.x + r.width, r.y), None);
    }
    assert_eq!(status::BACKGROUND, [12, 19, 31]);
    assert_eq!(status::TEXT, [214, 225, 244]);
}
