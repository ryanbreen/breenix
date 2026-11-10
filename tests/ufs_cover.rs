#[path = "../kernel/src/fs/ufs/cover.rs"]
mod cover;
use cover::{enter, leave, Cover};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mount { Root, Home }

#[test]
fn enter_by_inode_and_leave_to_the_covered_parent() {
    let covers = [Cover { mount: 7, covered: (Mount::Root, 42), parent: (Mount::Root, 9) }];
    assert_eq!(enter(covers.iter(), (Mount::Root, 42)), Some(7));
    assert_eq!(enter(covers.iter(), (Mount::Home, 42)), None);
    // namei replaces the covered directory's frame with node 1; popping it
    // on '..' must reach the covered directory's parent, not the directory.
    assert_eq!(leave(covers.iter(), 7), Some((Mount::Root, 9)));
}

#[test]
fn umount_removes_the_cover() {
    let mut covers = vec![Cover { mount: 7, covered: (Mount::Home, 42), parent: (Mount::Home, 2) }];
    assert_eq!(enter(covers.iter(), (Mount::Home, 42)), Some(7));
    covers.remove(0);
    assert_eq!(enter(covers.iter(), (Mount::Home, 42)), None);
    assert_eq!(leave(covers.iter(), 7), None);
}
