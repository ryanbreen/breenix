//! Stable-id gesture calculations and owner-mediated resize requests.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum Edge { Top, Bottom, Left, Right, TopLeft, TopRight, BottomLeft, BottomRight }

pub(super) struct Drag { id: Option<u64>, offset: (i32, i32) }
impl Drag {
    pub(super) fn new(id: u64, point: (i32, i32), origin: (i32, i32)) -> Self {
        Self { id: Some(id), offset: (point.0 - origin.0, point.1 - origin.1) }
    }
    pub(super) fn id(&self) -> Option<u64> { self.id }
    pub(super) fn motion(&mut self, ids: &[u64], point: (i32, i32)) -> Option<(usize, (i32, i32))> {
        let index = ids.iter().position(|id| Some(*id) == self.id);
        if index.is_none() { self.id = None; }
        index.map(|i| (i, (point.0 - self.offset.0, point.1 - self.offset.1)))
    }
}

pub(super) struct Resize { pub id: u64, edge: Edge, anchor: (i32, i32), pub origin: (i32, i32, usize, usize) }
impl Resize {
    pub(super) fn new(id: u64, edge: Edge, anchor: (i32, i32), origin: (i32, i32, usize, usize)) -> Self {
        Self { id, edge, anchor, origin }
    }
    pub(super) fn size(&self, point: (i32, i32), scale: u32, screen_w: usize, screen_h: usize) -> (u32, u32) {
        let (x, y, w, h) = self.origin;
        let (dx, dy) = (point.0 - self.anchor.0, point.1 - self.anchor.1);
        let mut width = w as i32;
        let mut height = h as i32;
        match self.edge {
            Edge::Right => width += dx, Edge::Left => width -= dx,
            Edge::Bottom => height += dy, Edge::Top => height -= dy,
            Edge::TopLeft => { width -= dx; height -= dy; },
            Edge::TopRight => { width += dx; height -= dy; },
            Edge::BottomLeft => { width -= dx; height += dy; },
            Edge::BottomRight => { width += dx; height += dy; },
        }
        // Left/top gestures keep the far edge fixed; right/bottom gestures
        // keep the origin fixed. An untouched axis keeps its current size,
        // including when a dragged window extends beyond the desktop.
        if matches!(self.edge, Edge::Left | Edge::TopLeft | Edge::BottomLeft) {
            width = width.min(x + w as i32).max((120 * scale) as i32);
        } else if matches!(self.edge, Edge::Right | Edge::TopRight | Edge::BottomRight) {
            width = width.min(screen_w as i32 - x.max(0)).max((120 * scale) as i32);
        }
        if matches!(self.edge, Edge::Top | Edge::TopLeft | Edge::TopRight) {
            height = height.min(y + h as i32 - (28 * scale) as i32).max((116 * scale) as i32);
        } else if matches!(self.edge, Edge::Bottom | Edge::BottomLeft | Edge::BottomRight) {
            height = height.min(screen_h as i32 - (36 * scale) as i32 - y).max((116 * scale) as i32);
        }
        ((width - (4 * scale) as i32).clamp(1, 4096) as u32,
         (height - (36 * scale) as i32).clamp(1, 4096) as u32)
    }
}

#[derive(Default)]
pub(super) struct ResizeQueue { pending: Option<(u32, u32)>, desired: Option<(u32, u32)> }
impl ResizeQueue {
    pub(super) fn motion(&mut self, current: (u32, u32), candidate: (u32, u32)) -> Option<(u32, u32)> {
        if let Some(pending) = self.pending {
            self.desired = (candidate != pending).then_some(candidate);
            return None;
        }
        if candidate == current { self.desired = None; return None; }
        self.pending = Some(candidate);
        Some(candidate)
    }
    pub(super) fn observed(&mut self, old: (u32, u32), current: (u32, u32)) -> Option<(u32, u32)> {
        if old == current { return None; }
        self.pending = None;
        self.desired.take().filter(|size| *size != current).inspect(|size| self.pending = Some(*size))
    }
}
