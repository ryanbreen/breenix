//! Geometry handed to coverage_spans, independently specified at sub-scanline centres.
use libfont::{crossings::Crossings, outline::LineSegment, work};

#[test]
fn scanlines_preserve_binary32_coordinates_equal_x_order_and_counted_sort() {
    let edges = [
        LineSegment { x0: 2.0, y0: 0.0, x1: 4.0, y1: 1.0 },
        LineSegment { x0: 2.0, y0: 1.0, x1: 4.0, y1: 0.0 },
        LineSegment { x0: 2.0, y0: 0.0, x1: 4.0, y1: 1.0 },
        LineSegment { x0: 7.0, y0: 0.0, x1: 7.0, y1: 0.0 },
    ];
    let mut crossings = Crossings::new(edges.len());
    work::reset();
    let first = crossings.scanline(&edges, 0, 0);
    assert_eq!(first.iter().map(|c| (c.x.to_bits(), c.direction)).collect::<Vec<_>>(),
        vec![(2.125f32.to_bits(), 1), (2.125f32.to_bits(), 1), (3.875f32.to_bits(), -1)]);
    // Four edge visits plus the sub-scanline visit, two counted merge passes,
    // then three crossing visits. No coverage-column or span-fill work occurs.
    assert_eq!(work::steps(), 5 + 2 * 4 + 3);
    assert!(crossings.scanline(&edges, 1, 0).is_empty());
    let reversed = [edges[1], edges[0], edges[2]];
    let middle = crossings.scanline(&reversed, 0, 3);
    assert_eq!(middle.iter().map(|c| (c.x.to_bits(), c.direction)).collect::<Vec<_>>(),
        vec![(2.875f32.to_bits(), 1), (2.875f32.to_bits(), 1), (3.125f32.to_bits(), -1)]);
    // Coincident crossings retain source order, including opposite directions.
    let same = [
        LineSegment { x0: 3.0, y0: 1.0, x1: 3.0, y1: 0.0 },
        LineSegment { x0: 3.0, y0: 0.0, x1: 3.0, y1: 1.0 },
    ];
    assert_eq!(crossings.scanline(&same, 0, 7).iter().map(|c| c.direction).collect::<Vec<_>>(), vec![-1, 1]);
}
