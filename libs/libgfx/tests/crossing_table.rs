//! Crossing geometry sent to coverage_spans, independently decoded as the frozen wire layout.
use libfont::outline::LineSegment;
use libgfx::ttf_font::crossing_table;
#[test]
fn table_keeps_eight_sorted_subscans_and_absolute_row_positions() {
    let edges = [
        LineSegment { x0: 3.75, y0: 0.25, x1: 3.75, y1: 3.75 },
        LineSegment { x0: 0.25, y0: 3.75, x1: 0.25, y1: 0.25 },
    ];
    let bytes = crossing_table(&edges, 1, 1);
    let words: Vec<_> = bytes.chunks_exact(4).map(|b| u32::from_le_bytes(b.try_into().unwrap())).collect();
    assert_eq!(&words[..9], &[0,2,4,6,8,10,12,14,16]);
    for pair in words[9..].chunks_exact(4) {
        assert_eq!(pair, &[0.25f32.to_bits(), (-1i32) as u32, 3.75f32.to_bits(), 1]);
    }
    let bytes = crossing_table(&edges, 4, 1);
    assert_eq!(bytes, vec![0;36]);
}
