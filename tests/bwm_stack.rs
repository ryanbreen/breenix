#[path = "../userspace/programs/src/bwm/stack.rs"]
mod stack;
use stack::Stack;

#[test]
fn adopt_restores_published_stack_and_later_add_focuses_on_top() {
    let mut s = Stack::new();
    s.adopt(&[(20, 1, true, false, false), (10, 0, false, true, false)]);
    assert_eq!(s.state(10), Some((0, false, true)));
    assert_eq!(s.state(20), Some((1, true, false)));
    assert_eq!(s.add(30, false), (Some(10), Some(30)));
    assert_eq!(s.state(30), Some((2, false, true)));
}

#[test]
fn raise_uses_ids_and_keeps_chromeless_above_frames() {
    let mut s = Stack::new();
    s.add(1, false); s.add(2, false); s.add(3, true);
    assert_eq!(s.raise(1), (Some(3), Some(1)));
    assert_eq!(s.order(), &[2, 1, 3]);
    s.raise(2);
    assert_eq!(s.order(), &[1, 2, 3]);
    assert_eq!(s.focus(), Some(2));
}

#[test]
fn minimize_restore_and_all_minimized() {
    let mut s = Stack::new();
    s.add(1, false); s.add(2, false);
    assert_eq!(s.minimize(2), (Some(2), Some(1)));
    assert!(s.minimized(2));
    assert_eq!(s.minimize(1), (Some(1), None));
    assert_eq!(s.focus(), None);
    assert_eq!(s.restore(2), (None, Some(2)));
    assert!(!s.minimized(2));
}

#[test]
fn remove_keeps_survivor_order_and_focus_is_unique() {
    let mut s = Stack::new();
    s.add(1, false); s.add(2, false); s.add(3, true);
    s.raise(1);
    assert_eq!(s.remove(1), (Some(1), Some(3)));
    assert_eq!(s.order(), &[2, 3]);
    assert_eq!(s.order().iter().filter(|&&id| s.focus() == Some(id) && !s.minimized(id)).count(), 1);
    s.minimize(3); s.minimize(2);
    assert_eq!(s.order().iter().filter(|&&id| s.focus() == Some(id) && !s.minimized(id)).count(), 0);
}

#[test]
fn published_state_follows_order_and_focus_through_minimize_and_exit() {
    let mut s = Stack::new();
    s.add(10, false);
    s.add(20, true);
    s.add(30, false);
    assert_eq!(s.order(), &[10, 30, 20]);
    assert_eq!(s.state(30), Some((1, false, true)));
    assert_eq!(s.state(20), Some((2, false, false)));
    assert_eq!(s.minimize(10), (Some(30), Some(30)));
    assert_eq!(s.state(10), Some((0, true, false)));
    assert_eq!(s.minimize(30), (Some(30), Some(20)));
    assert_eq!(s.state(20), Some((2, false, true)));
    assert_eq!(s.minimize(20), (Some(20), None));
    for &id in s.order() {
        let (_, minimized, focused) = s.state(id).unwrap();
        assert!(minimized);
        assert!(!focused);
    }
    assert_eq!(s.restore(10), (None, Some(10)));
    assert_eq!(s.order(), &[30, 10, 20]);
    assert_eq!(s.remove(30), (Some(10), Some(10)));
    assert_eq!(s.order(), &[10, 20]);
    assert_eq!(s.state(10), Some((0, false, true)));
    assert_eq!(s.state(20), Some((1, true, false)));
    assert_eq!(s.state(30), None);
    assert_eq!(s.raise(99), (Some(10), Some(10)));
    assert_eq!(s.order().iter().filter(|&&id| s.state(id).unwrap().2).count(), 1);
}

#[test]
fn focus_notices_surround_publication() {
    use stack::{publish_focus, FocusNotice};
    use std::cell::RefCell;
    let calls = RefCell::new(Vec::new());
    publish_focus((Some(1), Some(2)), |notice| calls.borrow_mut().push(format!("{notice:?}")), || calls.borrow_mut().push("publish".into()));
    assert_eq!(*calls.borrow(), ["Lost(1)", "publish", "Gained(2)"]);
    calls.borrow_mut().clear();
    publish_focus((None, Some(1)), |notice| calls.borrow_mut().push(format!("{notice:?}")), || calls.borrow_mut().push("publish".into()));
    assert_eq!(*calls.borrow(), ["publish", "Gained(1)"]);
    calls.borrow_mut().clear();
    publish_focus((Some(1), Some(1)), |notice: FocusNotice| calls.borrow_mut().push(format!("{notice:?}")), || calls.borrow_mut().push("publish".into()));
    assert_eq!(*calls.borrow(), ["publish"]);
}

#[test]
fn cycling_uses_creation_order_and_skips_minimized() {
    let mut s = Stack::new();
    s.add(1, false); s.add(2, false); s.add(3, false);
    s.minimize(2); s.raise(1);
    assert_eq!(s.cycle(false), Some(3));
    s.raise(3);
    assert_eq!(s.cycle(false), Some(1));
    assert_eq!(s.cycle(true), Some(1));
    s.raise(1);
    assert_eq!(s.cycle(true), Some(3));
    s.minimize(1); s.minimize(3);
    assert_eq!(s.cycle(false), None);
    assert_eq!(s.cycle(true), None);
    assert_eq!(s.focus(), None);
}

//! Desktop reporting from the production stack, plus live-page publication.
extern crate alloc;
extern crate self as breengel;
#[path = "../libs/libbui/src/tree.rs"]
pub mod tree;
#[path = "../libs/breengel/src/tree_page.rs"]
mod tree_page;
#[path = "../libs/breengel/src/tree_commit.rs"]
mod tree_commit;
#[path = "../userspace/programs/src/bwm/desktop_tree.rs"]
mod desktop_tree;
use desktop_tree::{build, WindowState};
use tree::{Flag, Role, Tree};
use std::sync::atomic::{AtomicU64, Ordering};

#[test]
fn canned_stack_reports_literal_windows_in_policy_order() {
    let mut stack = stack::Stack::new();
    stack.adopt(&[(7, 1, false, true, false), (42, 0, true, false, true)]);
    // Metadata discovery order intentionally differs from stacking order.
    let windows = [
        WindowState { id: 7, title: "Editor", owner: 81, bounds: [90, 70, 320, 200], chromeless: false },
        WindowState { id: 42, title: "\x01Palette", owner: 82, bounds: [-10, 20, 100, 80], chromeless: true },
    ];
    let tree = build(&stack, windows, [1280, 800], 91);
    assert_eq!(tree.root.key, "desktop");
    assert_eq!(tree.root.role, Role::Canvas);
    assert_eq!(tree.root.bounds, [0, 0, 1280, 800]);
    assert_eq!(tree.root.value, serde_json::json!({"scene_version":91}));
    assert_eq!(tree.children.len(), 2);
    let [lower, upper] = &tree.children[..] else { unreachable!() };
    assert_eq!((&*lower.key, lower.parent.as_deref(), lower.role, &*lower.name),
        ("window-42", Some("desktop"), Role::Window, "Palette"));
    assert_eq!(lower.value, serde_json::json!({"id":42,"owner":82,"z":0,"minimized":true,"chromeless":true}));
    assert_eq!(lower.bounds, [-10,20,100,80]);
    assert_eq!(lower.clip, [0,20,0,0]);
    assert_eq!(lower.flags, [Flag::Enabled]);
    assert_eq!((&*upper.key, upper.parent.as_deref(), upper.role, &*upper.name),
        ("window-7", Some("desktop"), Role::Window, "Editor"));
    assert_eq!(upper.value, serde_json::json!({"id":7,"owner":81,"z":1,"minimized":false,"chromeless":false}));
    assert_eq!(upper.bounds, [90,70,320,200]);
    assert_eq!(upper.clip, [90,70,320,200]);
    assert_eq!(upper.flags, [Flag::Enabled, Flag::Visible, Flag::Focused]);
    let mut bytes = vec![0; 65528];
    tree.write(&mut bytes).unwrap();
    assert_eq!(Tree::parse(&bytes).unwrap(), tree);
}

#[test]
fn live_page_refuses_unpublished_and_in_progress_data_and_gates_work() {
    let mut backing = vec![0u64; 8192];
    let page = unsafe { tree_page::Page::from_mapping(backing.as_mut_ptr().cast(), backing.len()*8).unwrap() };
    assert_eq!(page.snapshot().unwrap(), None);
    let mut count = 0;
    tree_commit::serialize_for_commit(0, &mut count, || panic!("unattached work")).unwrap();
    assert_eq!(count, 0);
    let stack = stack::Stack::new();
    let first = build(&stack, [], [800,600], 5);
    tree_commit::serialize_for_commit(1, &mut count, || unsafe { page.publish(&first) }).unwrap();
    assert_eq!(count, 1);
    assert_eq!(page.snapshot().unwrap(), Some(first));
    // Force an interleaving: writer started, previous valid JSON still present.
    let sequence = unsafe { &*(backing.as_ptr().cast::<AtomicU64>()) };
    assert_eq!(sequence.load(Ordering::SeqCst), 2);
    sequence.store(3, Ordering::SeqCst);
    assert_eq!(page.snapshot().unwrap(), None);
    sequence.store(2, Ordering::SeqCst);
    let next = build(&stack, [], [800,600], 6);
    unsafe { page.publish(&next).unwrap(); }
    assert_eq!(sequence.load(Ordering::SeqCst), 4);
    assert_eq!(page.snapshot().unwrap(), Some(next));
}
