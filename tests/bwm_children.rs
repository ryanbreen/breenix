#[path = "../userspace/programs/src/bwm/children.rs"]
mod children;

use children::{Children, Composed, Listed, Wait};

#[test]
fn reaps_all_exits_and_untracks_pending_launches() {
    let mut children = Children::default();
    children.track(11, 100);
    children.track(12, 100);
    let mut exits = vec![11, 99, 12];
    let mut reaped = Vec::new();
    let mut interrupted = true;
    children.reap(|| {
        if interrupted { interrupted = false; return Wait::Interrupted; }
        match exits.pop() {
        Some(pid) => { reaped.push(pid); Wait::Exited(pid) }
        None => Wait::Empty,
        }
    });
    assert_eq!(reaped, vec![12, 99, 11]);
    assert!(exits.is_empty());
    assert!(children.is_empty());
}

#[test]
fn acknowledges_composed_first_commit_for_listed_owner() {
    let mut children = Children::default();
    children.track(42, 100);
    assert_eq!(children.deadline(), Some(5_000_000_100));
    let frame = [Composed { id: 7, commit: 1, pixels: 1 }];
    assert!(children.acknowledge(&[Listed { id: 7, owner: 99, commit: 2 }], &frame).is_empty());
    assert_eq!(children.acknowledge(&[Listed { id: 7, owner: 42, commit: 2 }], &frame), vec![42]);
    assert!(children.acknowledge(&[Listed { id: 7, owner: 42, commit: 2 }], &frame).is_empty());
    assert!(children.is_empty());
}
