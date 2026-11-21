//! Production statfs dispatch with a local namespace and UFS transport fixture.
pub use crate::{mount_table, syscall_errno as errno, vfs::FileSystem};
#[path = "../../../kernel/src/fs/vfs/mount.rs"]
pub mod mount;

pub static TEST_MOUNTS: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub mod namei {
    #[derive(Clone, Copy)]
    pub struct Mount(pub usize);
    #[derive(Clone, Copy)]
    pub enum Target {
        Inode { mount: Mount, ino: u64 },
        User { mount: u64, node: u64 },
        Virtual,
        Absent { mount: Mount },
    }
    pub struct Resolved<P> {
        pub path: P,
        pub target: Target,
        pub virtual_absent: bool,
    }
}
pub mod ufs {
    pub mod path {
        pub fn request(
            _mount: u64,
            _op: ufs_abi::Op,
            _node: u64,
            _input: &[u8],
        ) -> Result<Vec<u8>, u64> {
            Ok([4096u64, 100, 80, 70, 50, 40, 255]
                .into_iter()
                .flat_map(u64::to_le_bytes)
                .collect())
        }
    }
    pub mod mounts_line {
        pub fn format_mount_line(_source: &[u8], _path: &[u8]) -> Vec<u8> {
            Vec::new()
        }
    }
}

mod dispatch {
    use super::{mount, namei};
    include!(concat!(env!("OUT_DIR"), "/statfs_dispatch.rs"));

    #[test]
    fn statfs_reports_distinct_nonzero_mount_ids() {
        let _registry = super::TEST_MOUNTS.lock().unwrap();
        use crate::{
            adapter::Ext2Adapter, attrs::MountFlags, host_mounts::Fixture, mount_table::Entry,
        };
        use namei::{Mount, Resolved, Target};
        use std::sync::Arc;

        let disks = [Fixture::new(), Fixture::new()];
        let mut mounts = Vec::new();
        for (path, disk) in [("/", &disks[0]), ("/second", &disks[1])] {
            let id = mount::allocate_id().unwrap();
            let fs = Arc::new(Ext2Adapter::new(disk.pin()));
            mount::register(Entry::new(
                id,
                path.as_bytes().to_vec(),
                "ext2",
                MountFlags::default(),
                mount::Backend::Fs(fs),
                2,
                None,
                None,
            ))
            .unwrap();
            mounts.push((
                path,
                id,
                Target::Inode {
                    mount: Mount(id),
                    ino: 2,
                },
                0xef53,
            ));
        }
        let user = mount::allocate_id().unwrap();
        mount::register_user(Entry::new(
            user,
            b"/ufs".to_vec(),
            "ufs",
            MountFlags::default(),
            mount::Backend::User(user as u64),
            1,
            None,
            None,
        ))
        .unwrap();
        mounts.push((
            "/ufs",
            user,
            Target::User {
                mount: user as u64,
                node: 1,
            },
            ufs_abi::payload::MAGIC as u64,
        ));
        for (path, kind, magic) in [("/dev", "devfs", 0x1373), ("/proc", "procfs", 0x9fa0)] {
            let id = mount::mount(path, kind);
            mounts.push((path, id, Target::Virtual, magic));
        }

        let mut fsids = Vec::new();
        for (path, id, target, magic) in &mounts {
            let resolved = Resolved {
                path: path.as_bytes().to_vec(),
                target: *target,
                virtual_absent: false,
            };
            let stat = resolved_statfs(&resolved).unwrap();
            eprintln!(
                "{path}: mount={id} type={:#x} fsid={:?}",
                stat.kind, stat.fsid
            );
            assert_eq!(stat.kind, *magic, "backend type for {path}");
            fsids.push(stat.fsid);
        }
        assert!(
            fsids.iter().all(|id| *id != [0, 0]),
            "mounted filesystem returned zero: {fsids:?}"
        );
        assert!(
            mounts.iter().all(|(_, id, _, _)| *id != 0),
            "mount ids must start above zero"
        );
        for ((path, id, _, _), fsid) in mounts.iter().zip(&fsids) {
            assert_eq!(
                *fsid,
                [*id as i32, ((*id as u64) >> 32) as i32],
                "table identity for {path}"
            );
        }
        for (index, fsid) in fsids.iter().enumerate() {
            assert!(
                !fsids[..index].contains(fsid),
                "mounts share an fsid: {fsids:?}"
            );
        }
        for (path, id, _, _) in mounts.into_iter().rev() {
            mount::unmount(id, true).unwrap_or_else(|e| panic!("unmount {path}: {e:?}"));
        }
    }
}
