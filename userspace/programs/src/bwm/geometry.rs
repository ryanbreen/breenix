// ─── Resize Edge ────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq)]
pub(super) enum ResizeEdge {
    Top,
    Bottom,
    Left,
    Right,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}
