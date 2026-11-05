#[path = "../userspace/programs/src/bwm/stack.rs"]
mod stack;
#[path = "../userspace/programs/src/bwm/pointer_route.rs"]
mod pointer_route;

use pointer_route::{Action, Axis, Content, Notice, NoticeKind, Router};
use stack::Stack;

fn content(id: u64, x: i32, y: i32) -> Content {
    Content { id, x, y, width: 100, height: 80 }
}
fn scene() -> (Stack, Vec<Content>, Router) {
    let mut stack = Stack::new();
    stack.add(10, false);
    stack.add(20, false);
    (stack, vec![content(20, 50, 40), content(10, 10, 20)], Router::new())
}
fn button(code: u16, value: i32) -> Action { Action::Button { code, value } }
fn has(notices: &[Notice], id: u64, kind: NoticeKind, x: i32, y: i32) -> bool {
    notices.contains(&Notice { id, kind, x, y })
}

#[test]
fn motion_hits_topmost_visible_content_in_stack_order() {
    let (mut s, windows, mut r) = scene();
    let out = r.route(&mut s, &windows, 1, (60, 50), true, Action::Move);
    assert!(has(&out.notices, 20, NoticeKind::Move, 10, 10));
    assert_eq!(s.focus(), Some(20));
    s.minimize(20);
    let out = r.route(&mut s, &windows, 1, (60, 50), true, Action::Move);
    assert!(has(&out.notices, 10, NoticeKind::Move, 50, 30));
    s.restore(20);
    s.raise(10);
    let out = r.route(&mut s, &windows, 1, (60, 50), true, Action::Move);
    assert!(has(&out.notices, 10, NoticeKind::Move, 50, 30));
}

#[test]
fn content_press_focuses_and_raises_with_u4_notice_order() {
    let (mut s, windows, mut r) = scene();
    let out = r.route(&mut s, &windows, 1, (20, 30), true, button(0x110, 1));
    assert_eq!(s.order(), &[20, 10]);
    assert_eq!(s.state(10), Some((1, false, true)));
    let calls = std::cell::RefCell::new(Vec::new());
    stack::publish_focus(out.focus.unwrap(),
        |n| calls.borrow_mut().push(format!("{n:?}")),
        || calls.borrow_mut().push("publish".into()));
    assert_eq!(*calls.borrow(), ["Lost(20)", "publish", "Gained(10)"]);
    assert!(has(&out.notices, 10, NoticeKind::Button { code: 0x110, pressed: true }, 10, 10));
    assert_eq!(s.cycle(false), Some(20));
}

#[test]
fn grab_routes_motion_and_release_outside_content_in_local_coordinates() {
    let (mut s, mut windows, mut r) = scene();
    r.route(&mut s, &windows, 1, (20, 30), true, button(0x110, 1));
    assert!(r.grabbed(1));
    windows[1].x = 30;
    let out = r.route(&mut s, &windows, 1, (5, 200), true, Action::Move);
    assert!(has(&out.notices, 10, NoticeKind::Move, -25, 180));
    let out = r.route(&mut s, &windows, 1, (5, 200), true, button(0x110, 0));
    assert!(has(&out.notices, 10, NoticeKind::Button { code: 0x110, pressed: false }, -25, 180));
    let out = r.route(&mut s, &windows, 1, (5, 201), true, Action::Move);
    assert!(out.notices.is_empty());
    assert!(!r.grabbed(1));
}

#[test]
fn motion_outside_content_without_grab_has_no_client_target() {
    let (mut s, windows, mut r) = scene();
    assert!(r.route(&mut s, &windows, 1, (500, 500), true, Action::Move).notices.is_empty());
}

#[test]
fn grabs_are_independent_per_device() {
    let (mut s, windows, mut r) = scene();
    r.route(&mut s, &windows, 1, (20, 30), true, button(0x110, 1));
    r.route(&mut s, &windows, 2, (140, 100), true, button(0x110, 1));
    let out = r.route(&mut s, &windows, 1, (500, 500), true, Action::Move);
    assert!(has(&out.notices, 10, NoticeKind::Move, 490, 480));
    let out = r.route(&mut s, &windows, 2, (500, 500), true, Action::Move);
    assert!(has(&out.notices, 20, NoticeKind::Move, 450, 460));
}

#[test]
fn right_button_routes_like_left() { secondary_button(0x111); }
#[test]
fn middle_button_routes_like_left() { secondary_button(0x112); }
fn secondary_button(code: u16) {
    let (mut s, windows, mut r) = scene();
    let out = r.route(&mut s, &windows, 1, (20, 30), true, button(code, 1));
    assert_eq!(s.focus(), Some(10));
    assert!(has(&out.notices, 10, NoticeKind::Button { code, pressed: true }, 10, 10));
    let out = r.route(&mut s, &windows, 1, (500, 500), true, button(code, 0));
    assert!(has(&out.notices, 10, NoticeKind::Button { code, pressed: false }, 490, 480));
}

#[test]
fn additional_button_stays_on_device_grab_until_last_release() {
    let (mut s, windows, mut r) = scene();
    r.route(&mut s, &windows, 1, (20, 30), true, button(0x110, 1));
    let out = r.route(&mut s, &windows, 1, (140, 100), true, button(0x111, 1));
    assert!(has(&out.notices, 10, NoticeKind::Button { code: 0x111, pressed: true }, 130, 80));
    r.route(&mut s, &windows, 1, (140, 100), true, button(0x110, 0));
    assert!(has(&r.route(&mut s, &windows, 1, (500, 500), true, Action::Move).notices,
        10, NoticeKind::Move, 490, 480));
    r.route(&mut s, &windows, 1, (500, 500), true, button(0x111, 0));
    assert!(r.route(&mut s, &windows, 1, (500, 500), true, Action::Move).notices.is_empty());
}

#[test]
fn wheel_uses_pointer_target_even_with_focus_and_grab_elsewhere() {
    let (mut s, windows, mut r) = scene();
    r.route(&mut s, &windows, 1, (20, 30), true, button(0x110, 1));
    let out = r.route(&mut s, &windows, 1, (140, 100), true, Action::Wheel(-3));
    assert!(has(&out.notices, 20, NoticeKind::Wheel(-3), 90, 60));
    assert_eq!(s.focus(), Some(10));
    let out = r.route(&mut s, &windows, 1, (500, 500), true, Action::Wheel(1));
    assert!(!out.notices.iter().any(|n| matches!(n.kind, NoticeKind::Wheel(_))));
}

#[test]
fn hover_sends_leave_then_enter_only_when_target_changes() {
    let (mut s, windows, mut r) = scene();
    let out = r.route(&mut s, &windows, 1, (20, 30), true, Action::Move);
    assert_eq!(out.notices[0], Notice { id: 10, kind: NoticeKind::Enter, x: 10, y: 10 });
    let out = r.route(&mut s, &windows, 1, (21, 31), true, Action::Move);
    assert_eq!(out.notices.len(), 1);
    let out = r.route(&mut s, &windows, 1, (140, 100), true, Action::Move);
    assert_eq!(&out.notices[..2], &[
        Notice { id: 10, kind: NoticeKind::Leave, x: 130, y: 80 },
        Notice { id: 20, kind: NoticeKind::Enter, x: 90, y: 60 },
    ]);
    let out = r.route(&mut s, &windows, 1, (500, 500), true, Action::Move);
    assert_eq!(out.notices, [Notice { id: 20, kind: NoticeKind::Leave, x: 450, y: 460 }]);
}

#[test]
fn removal_cancels_grab_without_transferring_it_to_another_id() {
    let (mut s, windows, mut r) = scene();
    r.route(&mut s, &windows, 1, (20, 30), true, button(0x110, 1));
    s.remove(10);
    let notices = r.reconcile(&s, &windows);
    assert!(has(&notices, 10, NoticeKind::Cancel, 10, 10));
    assert!(r.reconcile(&s, &windows).is_empty());
    let out = r.route(&mut s, &windows, 1, (140, 100), true, button(0x110, 0));
    assert!(!out.notices.iter().any(|n| matches!(n.kind, NoticeKind::Button { .. })));
}

#[test]
fn minimizing_cancels_grab_and_release_does_not_reach_new_focus() {
    let (mut s, windows, mut r) = scene();
    r.route(&mut s, &windows, 1, (20, 30), true, button(0x110, 1));
    s.minimize(10);
    assert!(has(&r.reconcile(&s, &windows), 10, NoticeKind::Cancel, 10, 10));
    s.restore(10);
    let out = r.route(&mut s, &windows, 1, (20, 30), true, button(0x110, 0));
    assert!(!out.notices.iter().any(|n| matches!(n.kind, NoticeKind::Button { .. })));
}

#[test]
fn repeated_held_reports_do_not_restart_grab_or_raise() {
    let (mut s, windows, mut r) = scene();
    r.route(&mut s, &windows, 1, (20, 30), true, button(0x110, 1));
    s.raise(20);
    for value in [1, 2] {
        let out = r.route(&mut s, &windows, 1, (140, 100), true, button(0x110, value));
        assert!(out.focus.is_none());
        assert!(!out.notices.iter().any(|n| matches!(n.kind, NoticeKind::Button { .. })));
        assert_eq!(s.focus(), Some(20));
    }
    assert!(has(&r.route(&mut s, &windows, 1, (500, 500), true, Action::Move).notices,
        10, NoticeKind::Move, 490, 480));
}

#[test]
fn dropped_device_cancels_and_allows_a_fresh_press() {
    let (mut s, windows, mut r) = scene();
    r.route(&mut s, &windows, 1, (20, 30), true, button(0x110, 1));
    let out = r.route(&mut s, &windows, 1, (20, 30), true, Action::Cancel);
    assert!(has(&out.notices, 10, NoticeKind::Cancel, 10, 10));
    let out = r.route(&mut s, &windows, 1, (140, 100), true, button(0x110, 1));
    assert!(has(&out.notices, 20, NoticeKind::Button { code: 0x110, pressed: true }, 90, 60));
}

#[test]
fn chrome_and_appbar_block_new_routes_but_do_not_steal_a_grab() {
    let (mut s, windows, mut r) = scene();
    let out = r.route(&mut s, &windows, 1, (20, 30), false, button(0x110, 1));
    assert!(out.notices.is_empty());
    assert!(out.focus.is_none());
    r.route(&mut s, &windows, 1, (20, 30), false, button(0x110, 0));
    r.route(&mut s, &windows, 1, (20, 30), true, button(0x110, 1));
    let out = r.route(&mut s, &windows, 1, (5, 200), false, button(0x110, 0));
    assert!(has(&out.notices, 10, NoticeKind::Button { code: 0x110, pressed: false }, -5, 180));
}

#[test]
fn contiguous_axes_route_at_batch_final_position_without_losing_source_slots() {
    let positions = pointer_route::batch_positions(&[
        (1, Axis::AbsoluteX(60)), (1, Axis::AbsoluteY(50)), (1, Axis::Boundary),
        (1, Axis::RelativeX(5)), (1, Axis::RelativeY(-10)), (2, Axis::RelativeX(7)),
        (1, Axis::Boundary),
    ], (0, 0), (200, 200));
    assert_eq!(positions, [Some((60, 50)), Some((60, 50)), None,
        Some((65, 40)), Some((65, 40)), Some((72, 40)), None]);
    let (mut s, windows, mut r) = scene();
    for point in positions[..2].iter().flatten() {
        let out = r.route(&mut s, &windows, 1, *point, true, Action::Move);
        assert!(has(&out.notices, 20, NoticeKind::Move, 10, 10));
    }
}
