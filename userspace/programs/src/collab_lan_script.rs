//! The same four single-segment strokes are submitted by each fixture.
use libcollab::wire::DrawOp;

pub fn strokes() -> Vec<Vec<DrawOp>> {
    (0..4).map(|i| super::collab::stroke(0, 0, [30 + i * 40, 50, 180],
        &[(4 + i as i32 * 10, 5), (8 + i as i32 * 10, 40)])).collect()
}
