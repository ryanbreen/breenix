use crate::{adapter::Ext2Adapter, attrs::{InodeAttrs, MountFlags}, errno::Errno,
    host_mounts::Fixture, mount_table::{Entry, MountTable}, syscall_errno::EBUSY,
    vfs::{FileSystem, NewNode, RenameMode}};

fn add(table: &mut MountTable<()>, path: &[u8], flags: MountFlags,
    root: u64, cover: Option<(usize, u64)>, parent: Option<(usize, u64)>) -> usize {
    let id = table.allocate_id().unwrap();
    table.insert(Entry::new(id, path.to_vec(), "fixture", flags, (), root, cover, parent)).unwrap();
    id
}

#[test]
fn cover_enters_by_inode_after_parent_rename() {
    let disk = Fixture::new();
    let fs = Ext2Adapter::new(disk.pin());
    let root = fs.root().unwrap();
    let dir = |parent, name| fs.create(parent, name, NewNode::Directory {
        attrs: InodeAttrs::new(0o755, 0, 0) }).unwrap();
    let ancestor = dir(root, b"old");
    let covered = dir(ancestor, b"mount");
    let mut table = MountTable::new();
    let native = add(&mut table, b"/", MountFlags::default(), root, None, None);
    let user = add(&mut table, b"/old/mount", MountFlags::default(), 1,
        Some((native, covered)), Some((native, ancestor)));
    fs.rename(root, b"old", root, b"new", RenameMode::Replace).unwrap();
    let moved = fs.lookup(fs.lookup(root, b"new").unwrap(), b"mount").unwrap();
    assert_eq!(table.enter((native, moved)), Some((user, 1)));
    assert_eq!(table.get(user).unwrap().parent, Some((native, ancestor)));
}

#[test]
fn flags_at_takes_longest_whole_component() {
    let mut table = MountTable::new();
    let base = MountFlags::default();
    let home = MountFlags { nosuid: true, nodev: true, ..base };
    let nested = MountFlags { read_only: true, ..home };
    add(&mut table, b"/", base, 2, None, None);
    let home_id = add(&mut table, b"/home", home, 2, None, None);
    add(&mut table, b"/home/shared", nested, 2, None, None);
    assert_eq!(table.flags_at(b"/home/shared/file"), nested);
    assert_eq!(table.flags_at(b"/home/sharedly/file"), home);
    assert_eq!(table.flags_at(b"/home"), home);
    assert_eq!(table.flags_at(b"/homeward"), base);
    assert_eq!(table.find_mount(b"/home/file"), Some(home_id));
}

#[test]
fn busy_mount_refuses_umount() {
    let mut table = MountTable::new();
    let id = add(&mut table, b"/mnt", MountFlags::default(), 2, None, None);
    let held = table.pin(id).unwrap();
    assert!(matches!(table.remove(id, false), Err(Errno(EBUSY))));
    assert!(table.get(id).is_some());
    drop(held);
    assert!(table.remove(id, false).is_ok());
    assert!(table.get(id).is_none());
}

#[test]
fn bind_enters_target_directory() {
    let disk = Fixture::new();
    let fs = Ext2Adapter::new(disk.pin());
    let root = fs.root().unwrap();
    let directory = |name| fs.create(root, name, NewNode::Directory {
        attrs: InodeAttrs::new(0o755, 0, 0) }).unwrap();
    let target = directory(b"shared");
    let cover = directory(b"home");
    let child = fs.create(target, b"visible", NewNode::Regular {
        attrs: InodeAttrs::new(0o644, 0, 0) }).unwrap();
    let mut table = MountTable::new();
    let native = add(&mut table, b"/", MountFlags::default(), root, None, None);
    let bind = add(&mut table, b"/home", MountFlags::default(), target,
        Some((native, cover)), Some((native, root)));
    table.set_bind(bind, (native, target)).unwrap();
    let (entered, ino) = table.enter((native, cover)).unwrap();
    assert_eq!(entered, bind);
    assert_eq!(ino, target);
    assert_eq!(table.get(bind).unwrap().bind, Some((native, target)));
    assert_eq!(fs.lookup(ino, b"visible"), Ok(child));
    assert!(matches!(table.remove(native, false), Err(Errno(EBUSY))));
    table.remove(bind, false).unwrap();
    assert_eq!(table.enter((native, cover)), None);
}
