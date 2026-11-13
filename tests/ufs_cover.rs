extern crate alloc;
#[path = "../kernel/src/fs/attrs.rs"]
pub mod attrs;
#[path = "../kernel/src/fs/errno.rs"]
pub mod errno;
#[path = "../kernel/src/syscall/errno.rs"]
pub mod syscall_errno;
pub mod fs {
    pub use crate::{attrs, errno};
}
pub mod syscall {
    pub use crate::syscall_errno as errno;
}
#[path = "../kernel/src/fs/vfs/mount_table.rs"]
pub mod mount_table;
use mount_table::{Entry, MountTable};

fn covered() -> MountTable<()> {
    let mut table = MountTable::new();
    table
        .insert(Entry::new(
            7,
            b"/mnt".to_vec(),
            "ufs",
            attrs::MountFlags::default(),
            (),
            1,
            Some((0, 42)),
            Some((0, 9)),
        ))
        .unwrap();
    table
}

#[test]
fn enter_by_inode_and_leave_to_the_covered_parent() {
    let table = covered();
    assert_eq!(table.enter((0, 42)), Some((7, 1)));
    assert_eq!(table.enter((1, 42)), None);
    // Popping the daemon root reaches the covered directory's parent.
    assert_eq!(table.get(7).unwrap().parent, Some((0, 9)));
}

#[test]
fn umount_removes_the_cover() {
    let mut table = covered();
    assert_eq!(table.enter((0, 42)), Some((7, 1)));
    table.remove(7, false).unwrap();
    assert_eq!(table.enter((0, 42)), None);
    assert!(table.get(7).is_none());
}
