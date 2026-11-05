#[path = "../userspace/programs/src/bwm/gesture.rs"]
mod gesture;
use gesture::{Drag, Edge, Resize, ResizeQueue};

#[test]
fn drag_tracks_id_after_reorder_and_ends_on_removal() {
    let mut drag = Drag::new(10, (20, 30), (10, 10));
    let mut windows = [(10, (10, 10)), (20, (40, 40))];
    windows.swap(0, 1);
    let (index, position) = drag.motion(&[windows[0].0, windows[1].0], (30, 40)).unwrap();
    windows[index].1 = position;
    assert_eq!(windows, [(20, (40, 40)), (10, (20, 20))]);
    assert!(drag.motion(&[20], (40, 50)).is_none());
    assert!(drag.motion(&[10, 20], (40, 50)).is_none());
}

#[test]
fn resize_coalesces_until_list_changes_and_ignores_duplicates() {
    let mut queue = ResizeQueue::default();
    let current = (200, 100);
    assert_eq!(queue.motion(current, (210, 100)), Some((210, 100)));
    for width in 211..=220 { assert_eq!(queue.motion(current, (width, 100)), None); }
    assert_eq!(queue.observed(current, current), None);
    assert_eq!(queue.observed(current, (210, 100)), Some((220, 100)));
    assert_eq!(queue.motion((210, 100), (220, 100)), None);
    assert_eq!(queue.observed((210, 100), (220, 100)), None);
    assert_eq!(queue.motion((220, 100), (220, 100)), None);
}

#[test]
fn ignored_request_does_not_change_size_or_release_pending() {
    let mut queue = ResizeQueue::default();
    let current = (200, 100);
    assert_eq!(queue.motion(current, (250, 100)), Some((250, 100)));
    assert_eq!(queue.motion(current, (260, 100)), None);
    assert_eq!(queue.observed(current, current), None);
    assert_eq!(queue.motion(current, (270, 100)), None);
}

#[test]
fn new_gesture_recovers_after_an_owner_refuses_the_previous_request() {
    let mut queue = ResizeQueue::default();
    let current = (200, 100);
    assert_eq!(queue.motion(current, (250, 100)), Some((250, 100)));
    assert_eq!(queue.motion(current, (260, 100)), None);
    assert_eq!(queue.observed(current, current), None);
    queue.begin();
    // No size was fabricated, and neither the refused nor coalesced request
    // from the old gesture blocks or gets replayed into the new gesture.
    assert_eq!(queue.motion(current, (270, 100)), Some((270, 100)));
    assert_eq!(queue.observed(current, (270, 100)), None);
}

#[test]
fn crossing_opposite_edge_clamps_minimum_content_width() {
    for scale in [1, 2] {
        let resize = Resize::new(10, Edge::Right, (300, 100), (100, 50, 200, 150));
        assert_eq!(resize.size((0, 100), scale, 1000, 1000).0, 116 * scale);
        let left = Resize::new(10, Edge::Left, (100, 100), (100, 50, 200, 150));
        assert_eq!(left.size((500, 100), scale, 1000, 1000).0, 116 * scale);
    }
}

#[test]
fn other_edges_preserve_the_untouched_dimension() {
    let base = (100, 50, 200, 150);
    for edge in [Edge::Top, Edge::Bottom, Edge::TopLeft, Edge::TopRight, Edge::BottomLeft, Edge::BottomRight] {
        let resize = Resize::new(42, edge, (100, 100), base);
        assert_eq!(resize.id, 42);
        let (w, h) = resize.size((100, 100), 1, 1000, 1000);
        assert_eq!((w, h), (196, 114));
    }
    assert_eq!(Drag::new(42, (1, 1), (0, 0)).id(), Some(42));
}

#[test]
fn offscreen_and_near_boundary_resizes_use_the_fixed_opposite_edge() {
    for scale in [1, 2] {
        let s = scale as i32;
        let size = |edge, origin: (i32, i32, usize, usize), motion: (i32, i32)| {
            Resize::new(10, edge, (0, 0),
                (origin.0 * s, origin.1 * s, origin.2 * scale, origin.3 * scale))
                .size((motion.0 * s, motion.1 * s), scale as u32, 1920 * scale, 1080 * scale)
        };
        // A vertical edge must leave the horizontally offscreen window's width alone.
        assert_eq!(size(Edge::Top, (1800, 600, 600, 300), (0, -100)),
            (596 * scale as u32, 364 * scale as u32));
        assert_eq!(size(Edge::Bottom, (1800, 600, 600, 300), (0, 100)),
            (596 * scale as u32, 364 * scale as u32));
        assert_eq!(size(Edge::Left, (1500, 600, 400, 300), (-900, 0)),
            (1296 * scale as u32, 264 * scale as u32));
        assert_eq!(size(Edge::Top, (100, 600, 400, 300), (0, -500)),
            (396 * scale as u32, 764 * scale as u32));
        assert_eq!(size(Edge::TopLeft, (1500, 600, 400, 300), (-900, -500)),
            (1296 * scale as u32, 764 * scale as u32));
        // Moving edges still stop at the desktop bounds.
        assert_eq!(size(Edge::TopLeft, (1500, 600, 400, 300), (-2000, -1000)),
            (1896 * scale as u32, 836 * scale as u32));
        assert_eq!(size(Edge::BottomRight, (1500, 600, 400, 300), (1000, 1000)),
            (416 * scale as u32, 408 * scale as u32));
    }
}
