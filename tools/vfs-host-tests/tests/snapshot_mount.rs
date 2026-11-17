//! Named-root mounts use the kernel adapter, with the host writer only for setup.
use crate::{
    attrs::InodeAttrs,
    block::cache::BufferCache,
    breenfs::{
        commit::{Committer, DeviceStore},
        format::{Superblock, Time},
        ops::Files,
        vfs::BreenFs,
    },
    errno::Errno,
    read_conformance::{Disk, NoWait},
    syscall_errno::*,
    vfs::*,
};
use std::sync::{Arc, Mutex};

fn fixture() -> (Arc<Disk>, u64, u64) {
    let disk = Arc::new(Disk {
        bytes: Mutex::new(vec![0; 64 * 1024 * 1024]),
        writable: true,
    });
    let sb = Superblock {
        fs_uuid: [0xe5; 16], generation: 1, slot: 1, total_blocks: 16384,
        global_reserve: 4096, next_root_id: 2, ..Superblock::default()
    };
    let mut committer = Committer::format(sb.clone());
    let mut files = Files::format(DeviceStore(disk.as_ref()), &sb).unwrap();
    let ino = files.create(2, b"file", 0o644, Time::default()).unwrap();
    files.write(disk.as_ref(), ino, 0, b"frozen", Time::default()).unwrap();
    let sb = files.fsync(disk.as_ref(), &mut committer).unwrap();
    let mut files = Files::new(DeviceStore(disk.as_ref()), &sb, 1, true).unwrap();
    let snapshot = files.snapshot(b"frozen", true).unwrap();
    files.snapshot(b"work", false).unwrap();
    files.fsync(disk.as_ref(), &mut committer).unwrap();
    drop(files);
    (disk, ino, snapshot)
}

fn named(disk: Arc<Disk>, name: Option<&[u8]>, writable: bool, id: usize)
    -> Result<Arc<BreenFs<NoWait>>, Errno>
{
    BreenFs::mount_named(disk, Arc::new(BufferCache::new(NoWait)), id, name, writable)
}

#[test]
fn named_snapshot_stays_frozen_beside_a_writer_and_pins_its_root() {
    let (disk, ino, snapshot) = fixture();
    let writer = named(disk.clone(), Some(b"root=work"), true, 141).unwrap();
    assert_eq!(writer.check_root_unmounted(snapshot), Ok(()));
    let reader = named(disk.clone(), Some(b"root=frozen"), false, 142).unwrap();
    let second = named(disk.clone(), Some(b"root=frozen"), false, 143).unwrap();
    assert_eq!(writer.check_root_unmounted(snapshot), Err(Errno(EBUSY)));
    assert_eq!(reader.read_only(), Ok(true));
    assert!(reader.statfs().unwrap().flags.read_only);
    assert_eq!(reader.lookup(2, b"file"), Ok(ino));
    assert_eq!(reader.create(2, b"refused", NewNode::Regular {
        attrs: InodeAttrs::new(0o600, 0, 0),
    }), Err(Errno(EROFS)));
    let live = writer.open(ino).unwrap();
    // Advance beyond the two protected generations before the first data read.
    for n in 0..8 {
        writer.write(&live, WriteReq { at: WriteAt::Offset(0), limit: u64::MAX,
            clear_setid: false }, &[b'a' + n; 6]).unwrap();
        writer.sync().unwrap();
    }
    let held = reader.open(ino).unwrap();
    let mut bytes = [0; 6];
    assert_eq!(reader.read(&held, 0, &mut bytes), Ok(6));
    assert_eq!(&bytes, b"frozen");
    assert_eq!(writer.read(&live, 0, &mut bytes), Ok(6));
    assert_eq!(&bytes, b"hhhhhh");
    assert_eq!(named(disk.clone(), Some(b"root=default"), true, 144).err(), Some(Errno(EBUSY)));
    reader.shutdown("unmount", u64::MAX).unwrap();
    // A refused unmount/restart and a second reader retain the registry claim.
    assert_eq!(writer.check_root_unmounted(snapshot), Err(Errno(EBUSY)));
    reader.init_service().unwrap();
    drop(reader);
    assert_eq!(writer.check_root_unmounted(snapshot), Err(Errno(EBUSY)));
    drop(second);
    assert_eq!(writer.check_root_unmounted(snapshot), Err(Errno(EBUSY)));
    drop(held);
    assert_eq!(writer.check_root_unmounted(snapshot), Ok(()));
    drop(live);
    drop(writer);
    assert!(named(disk, Some(b"root=work"), true, 145).is_ok());
}

#[test]
fn named_mount_errors_leave_no_claim_and_default_data_still_works() {
    let (disk, _, snapshot) = fixture();
    for writable in [false, true] {
        assert_eq!(named(disk.clone(), Some(b"root=absent"), writable, 146).err(), Some(Errno(ENOENT)));
    }
    assert_eq!(named(disk.clone(), Some(b"root=frozen"), true, 146).err(), Some(Errno(EROFS)));
    assert_eq!(named(disk.clone(), Some(b"root="), false, 146).err(), Some(Errno(EINVAL)));
    for data in [None, Some(b"".as_slice()), Some(b"root=default".as_slice())] {
        let fs = named(disk.clone(), data, true, 147).unwrap();
        assert_eq!(fs.check_root_unmounted(snapshot), Ok(()));
        assert_eq!(fs.check_root_unmounted(1), Err(Errno(EBUSY)));
        assert_eq!(fs.read_only(), Ok(false));
    }
    let cache = Box::leak(Box::new(BufferCache::new(NoWait)));
    let fs = BreenFs::mount_named_cached(disk, cache, 148, Some(b"root=frozen"), false).unwrap();
    assert_eq!(fs.check_root_unmounted(snapshot), Err(Errno(EBUSY)));
    // Keep the disk source separate from filesystem-specific mount data.
    let mount = include_str!("../../../kernel/src/fs/breenfs/mount.rs");
    assert!(mount.contains("entry.source = source;"));
    let dispatch = include_str!("../../../kernel/src/fs/ufs/mod.rs");
    assert!(dispatch.contains("mount::mount(source, target, flags, data, &cred)"));
}
