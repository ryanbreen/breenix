//! Filesystem boundary shims for the production namespace table. The host
//! exercises socket binding/lookup policy; guest tests validate ext2/namei.
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard};

pub mod irq_safe_mutex {
    pub type IrqSafeMutex<T> = spin::Mutex<T>;
}
pub mod fs {
    pub mod ext2 {
        #[derive(Clone, Copy, PartialEq, Eq)]
        pub enum FileType {
            Socket,
            Regular,
        }
        pub mod live_inode {
            #[derive(Clone, Copy, PartialEq, Eq)]
            pub struct MountPin {
                pub mount_id: usize,
            }
            #[derive(Clone, Copy, PartialEq, Eq)]
            pub struct FileKey {
                pub inode: u32,
            }
            pub struct LiveInode {
                pub mount: MountPin,
                pub key: FileKey,
            }
            #[derive(Clone)]
            pub struct FileHandle {
                pub object: std::sync::Arc<LiveInode>,
            }
            impl FileHandle {
                pub fn verify(
                    &self,
                    _fs: &super::super::super::HostFs,
                ) -> Result<u32, &'static str> {
                    Ok(self.object.key.inode)
                }
            }
        }
    }
    pub mod permissions {
        pub struct Credentials {
            pub umask: u32,
        }
        impl Credentials {
            pub fn current(_real: bool) -> Self {
                Self { umask: 0o022 }
            }
            pub fn permits(&self, node: &super::super::Node, wanted: u32) -> bool {
                (node.mode >> 6) & wanted == wanted
            }
        }
    }
    pub mod namei {
        use super::ext2::{live_inode::FileHandle, FileType};
        #[derive(Clone, Copy)]
        pub struct Mount;
        impl Mount {
            pub fn read(self) -> Result<super::super::HostGuard, u64> {
                Ok(super::super::HostGuard(super::super::FS.lock().unwrap()))
            }
        }
        pub enum Target {
            Inode { mount: Mount, file_type: FileType },
            Virtual,
        }
        pub struct Resolved {
            pub target: Target,
            pub pin: FileHandle,
        }
        impl Resolved {
            pub fn handle(&self) -> Option<&FileHandle> {
                Some(&self.pin)
            }
        }
        pub fn fs_errno(_error: &'static str) -> u64 {
            5
        }
    }
}

#[derive(Clone)]
pub struct Node {
    pub handle: fs::ext2::live_inode::FileHandle,
    pub mode: u32,
}
pub struct HostFs {
    paths: BTreeMap<String, Node>,
    next: u32,
}
pub struct HostGuard(MutexGuard<'static, HostFs>);
impl HostGuard {
    pub fn as_ref(&self) -> Option<&HostFs> {
        Some(&self.0)
    }
}
impl HostFs {
    pub fn read_inode(&self, ino: u32) -> Result<Node, &'static str> {
        self.paths
            .values()
            .find(|n| n.handle.object.key.inode == ino)
            .cloned()
            .ok_or("absent")
    }
}
static FS: Mutex<HostFs> = Mutex::new(HostFs {
    paths: BTreeMap::new(),
    next: 1,
});
pub mod at_path {
    pub const AT_FDCWD: i32 = -100;
}
pub mod metadata {
    use super::*;
    pub fn resolve_at(
        _fd: i32,
        path: &str,
        _follow: bool,
        _cred: &fs::permissions::Credentials,
    ) -> Result<fs::namei::Resolved, u64> {
        let fs = FS.lock().unwrap();
        let node = fs.paths.get(path).ok_or(2u64)?;
        Ok(fs::namei::Resolved {
            target: fs::namei::Target::Inode {
                mount: fs::namei::Mount,
                file_type: if node.mode & 0o170000 == 0o140000 {
                    fs::ext2::FileType::Socket
                } else {
                    fs::ext2::FileType::Regular
                },
            },
            pin: node.handle.clone(),
        })
    }
}
pub mod fifo {
    use super::*;
    pub fn mknodat_path(
        _fd: i32,
        path: &str,
        mode: u32,
        _dev: u64,
    ) -> crate::syscall::SyscallResult {
        let mut fs = FS.lock().unwrap();
        if fs.paths.contains_key(path) {
            return crate::syscall::SyscallResult::Err(17);
        }
        let inode = fs.next;
        fs.next += 1;
        let handle = fs::ext2::live_inode::FileHandle {
            object: Arc::new(fs::ext2::live_inode::LiveInode {
                mount: fs::ext2::live_inode::MountPin { mount_id: 1 },
                key: fs::ext2::live_inode::FileKey { inode },
            }),
        };
        fs.paths.insert(path.into(), Node { handle, mode });
        crate::syscall::SyscallResult::Ok(0)
    }
}

#[test]
fn abstract_bind_uses_every_byte_and_releases_by_socket_identity() {
    use crate::{addr::UnixName, socket::object::Socket, unix_ns::*};
    let a = Arc::new(Socket);
    let b = Arc::new(Socket);
    let short = UnixName::Abstract(b"ns-test-a".to_vec());
    let long = UnixName::Abstract(b"ns-test-a\0".to_vec());
    bind_name(&short, Arc::downgrade(&a)).unwrap();
    assert_eq!(bind_name(&short, Arc::downgrade(&b)), Err(98));
    bind_name(&long, Arc::downgrade(&b)).unwrap();
    assert!(Arc::ptr_eq(&lookup(&short).unwrap(), &a));
    assert!(Arc::ptr_eq(&lookup(&long).unwrap(), &b));
    unbind(&a);
    unbind(&b);
    assert!(matches!(lookup(&short), Err(111)));
    bind_name(&short, Arc::downgrade(&b)).unwrap();
    unbind(&b);
}

#[test]
fn pathname_nodes_are_exclusive_persist_on_close_and_unlink_hides_binding() {
    use crate::{addr::UnixName, socket::object::Socket, unix_ns::*};
    let a = Arc::new(Socket);
    let b = Arc::new(Socket);
    let path = UnixName::Path(b"/tmp/ns-test-path".to_vec());
    bind_name(&path, Arc::downgrade(&a)).unwrap();
    assert_eq!(bind_name(&path, Arc::downgrade(&b)), Err(98));
    assert!(Arc::ptr_eq(&lookup(&path).unwrap(), &a));
    let old = FS
        .lock()
        .unwrap()
        .paths
        .remove("/tmp/ns-test-path")
        .unwrap();
    assert!(matches!(lookup(&path), Err(2)));
    bind_name(&path, Arc::downgrade(&b)).unwrap();
    assert!(Arc::ptr_eq(&lookup(&path).unwrap(), &b));
    unbind(&a);
    assert!(Arc::ptr_eq(&lookup(&path).unwrap(), &b));
    unbind(&b);
    assert!(matches!(lookup(&path), Err(111)));
    assert_eq!(bind_name(&path, Arc::downgrade(&a)), Err(98));
    drop(old);
}

#[test]
fn ordinary_files_are_refused_and_socket_write_permission_is_checked() {
    use crate::{addr::UnixName, socket::object::Socket, unix_ns::*};
    let path = UnixName::Path(b"/tmp/ns-test-regular".to_vec());
    fifo::mknodat_path(-100, "/tmp/ns-test-regular", 0o100666, 0);
    assert!(matches!(lookup(&path), Err(111)));
    let a = Arc::new(Socket);
    let path = UnixName::Path(b"/tmp/ns-test-readonly".to_vec());
    bind_name(&path, Arc::downgrade(&a)).unwrap();
    FS.lock()
        .unwrap()
        .paths
        .get_mut("/tmp/ns-test-readonly")
        .unwrap()
        .mode = 0o140444;
    assert!(matches!(lookup(&path), Err(13)));
    unbind(&a);
}
