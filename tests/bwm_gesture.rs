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
fn crossing_opposite_edge_clamps_minimum_content_width() {
    for scale in [1, 2] {
        let resize = Resize::new(10, Edge::Right, (300, 100), (100, 50, 200, 150));
        assert_eq!(resize.size((0, 100), scale, 1000, 1000).0, 116 * scale);
        let left = Resize::new(10, Edge::Left, (100, 100), (100, 50, 200, 150));
        assert_eq!(left.size((500, 100), scale, 1000, 1000).0, 116 * scale);
    }
}
