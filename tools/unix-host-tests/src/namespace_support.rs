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
            pub euid: u32,
        }
        impl Credentials {
            pub fn current(_real: bool) -> Self {
                Self {
                    umask: 0o022,
                    euid: 0,
                }
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
            pub fn write(self) -> Result<super::super::HostGuard, u64> {
                self.read()
            }
            pub fn read(self) -> Result<super::super::HostGuard, u64> {
                Ok(super::super::HostGuard(super::super::FS.lock().unwrap()))
            }
        }
        pub enum Target {
            Inode { mount: Mount, file_type: FileType },
            Absent { mount: Mount },
            Virtual,
        }
        #[derive(PartialEq)]
        pub enum Last {
            Name,
        }
        pub struct Resolved {
            pub path: Vec<u8>,
            pub last: Last,
            pub trailing_slash: bool,
            pub target: Target,
            pub pin: Option<FileHandle>,
            pub virtual_absent: bool,
        }
        impl Resolved {
            pub fn entry_dir(&self) -> Option<&FileHandle> {
                self.pin.as_ref()
            }
            pub fn handle(&self) -> Option<&FileHandle> {
                self.pin.as_ref()
            }
        }
        pub fn resolve_socket_path(
            path: &[u8],
            create: bool,
            _cred: &super::permissions::Credentials,
        ) -> Result<Resolved, u64> {
            super::super::metadata::resolve_bytes(path, create)
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
    paths: BTreeMap<Vec<u8>, Node>,
    next: u32,
}
pub struct HostGuard(MutexGuard<'static, HostFs>);
impl HostGuard {
    pub fn as_mut(&mut self) -> Option<&mut HostFs> {
        Some(&mut self.0)
    }
    pub fn as_ref(&self) -> Option<&HostFs> {
        Some(&self.0)
    }
}
impl HostFs {
    pub fn read_inode(&self, ino: u32) -> Result<Node, &'static str> {
        if ino == 0 {
            return Ok(Node {
                handle: make_handle(0),
                mode: 0o40777,
            });
        }
        self.paths
            .values()
            .find(|n| n.handle.object.key.inode == ino)
            .cloned()
            .ok_or("absent")
    }
}
fn make_handle(inode: u32) -> fs::ext2::live_inode::FileHandle {
    fs::ext2::live_inode::FileHandle {
        object: Arc::new(fs::ext2::live_inode::LiveInode {
            mount: fs::ext2::live_inode::MountPin { mount_id: 1 },
            key: fs::ext2::live_inode::FileKey { inode },
        }),
    }
}
impl HostFs {
    pub fn pin_loaded_inode(
        &self,
        ino: u32,
        _size: u64,
    ) -> Result<fs::ext2::live_inode::FileHandle, &'static str> {
        Ok(self.read_inode(ino)?.handle)
    }
    pub fn create_node_prepared<R>(
        &mut self,
        _parent: u32,
        name: &[u8],
        mode: u16,
        _uid: u32,
        _gid: u32,
        _dev: u32,
        prepare: impl FnOnce(&mut Self, u32) -> Result<R, &'static str>,
    ) -> Result<R, &'static str> {
        if self.paths.contains_key(name) {
            return Err("already exists");
        }
        let ino = self.next;
        self.next += 1;
        self.paths.insert(
            name.to_vec(),
            Node {
                handle: make_handle(ino),
                mode: mode as u32,
            },
        );
        match prepare(self, ino) {
            Ok(result) => Ok(result),
            Err(e) => {
                self.paths.remove(name);
                Err(e)
            }
        }
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
    pub fn resolve_bytes(path: &[u8], create: bool) -> Result<fs::namei::Resolved, u64> {
        let fs = FS.lock().unwrap();
        // The shim uses the whole pathname as its final entry key, allowing
        // tests to observe exactly which byte string the production NS passes.
        let node = fs.paths.get(path.rsplit(|b| *b == b'/').next().unwrap());
        Ok(fs::namei::Resolved {
            path: path.rsplit(|b| *b == b'/').next().unwrap().to_vec(),
            last: fs::namei::Last::Name,
            trailing_slash: path.ends_with(b"/"),
            target: match node {
                Some(node) => fs::namei::Target::Inode {
                    mount: fs::namei::Mount,
                    file_type: if node.mode & 0o170000 == 0o140000 {
                        fs::ext2::FileType::Socket
                    } else {
                        fs::ext2::FileType::Regular
                    },
                },
                None => fs::namei::Target::Absent {
                    mount: fs::namei::Mount,
                },
            },
            pin: if create {
                Some(make_handle(0))
            } else {
                node.map(|n| n.handle.clone())
            },
            virtual_absent: false,
        })
    }
}
pub mod fifo_policy {
    pub fn check_parent_access(
        _inode: &super::Node,
        _cred: &super::fs::permissions::Credentials,
        _extra: Option<()>,
    ) -> Result<u32, u64> {
        Ok(0)
    }
    pub fn creation_mode(
        mode: u32,
        cred: &super::fs::permissions::Credentials,
        _gid: u32,
        _dir: bool,
    ) -> u16 {
        (mode & !cred.umask) as u16
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
        if fs.paths.contains_key(path.as_bytes()) {
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
        fs.paths.insert(
            path.as_bytes()
                .rsplit(|b| *b == b'/')
                .next()
                .unwrap()
                .to_vec(),
            Node { handle, mode },
        );
        crate::syscall::SyscallResult::Ok(0)
    }
}

#[test]
fn abstract_bind_uses_every_byte_and_releases_by_socket_identity() {
    use crate::{addr::UnixName, unix_ns::*};
    let a = crate::protocol_support::make_socket(1);
    let b = crate::protocol_support::make_socket(1);
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
    use crate::{addr::UnixName, unix_ns::*};
    let a = crate::protocol_support::make_socket(1);
    let b = crate::protocol_support::make_socket(1);
    let path = UnixName::Path(b"/tmp/ns-test-path".to_vec());
    bind_name(&path, Arc::downgrade(&a)).unwrap();
    assert_eq!(bind_name(&path, Arc::downgrade(&b)), Err(98));
    assert!(Arc::ptr_eq(&lookup(&path).unwrap(), &a));
    let old = FS
        .lock()
        .unwrap()
        .paths
        .remove(b"ns-test-path".as_slice())
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
    use crate::{addr::UnixName, unix_ns::*};
    let path = UnixName::Path(b"/tmp/ns-test-regular".to_vec());
    fifo::mknodat_path(-100, "/tmp/ns-test-regular", 0o100666, 0);
    assert!(matches!(lookup(&path), Err(111)));
    let a = crate::protocol_support::make_socket(1);
    let path = UnixName::Path(b"/tmp/ns-test-readonly".to_vec());
    bind_name(&path, Arc::downgrade(&a)).unwrap();
    FS.lock()
        .unwrap()
        .paths
        .get_mut(b"ns-test-readonly".as_slice())
        .unwrap()
        .mode = 0o140444;
    assert!(matches!(lookup(&path), Err(13)));
    unbind(&a);
}

#[test]
fn relocated_release_removes_the_original_inode_pin() {
    use crate::{
        addr::{SockAddr, UnixName},
        object::{Socket, SocketProto},
    };
    let socket = crate::protocol_support::make_socket(1);
    let name = SockAddr::Unix(UnixName::Path(b"/tmp/ns-relocated".to_vec()));
    socket.ops().bind(&socket, &name).unwrap();
    let inode = FS
        .lock()
        .unwrap()
        .paths
        .get(b"ns-relocated".as_slice())
        .unwrap()
        .handle
        .object
        .clone();
    assert_eq!(Arc::strong_count(&inode), 3); // filesystem, binding, observer
    let mut moved = Arc::try_unwrap(socket).ok().unwrap();
    let protocol = moved.proto.detach();
    drop(moved);
    assert_eq!(Arc::strong_count(&inode), 3);
    let carcass = Socket::new(1, 1, 0, protocol).unwrap();
    assert!(matches!(carcass.proto, SocketProto::Unix(_)));
    drop(carcass);
    assert_eq!(Arc::strong_count(&inode), 2); // binding released by original identity
}

#[test]
fn an_absent_final_component_is_enoent() {
    use crate::{addr::UnixName, unix_ns::lookup};
    assert!(matches!(
        lookup(&UnixName::Path(b"/tmp/ns-missing-final".to_vec())),
        Err(2)
    ));
}

#[test]
fn socket_paths_preserve_non_utf8_bytes() {
    use crate::{
        addr::{SockAddr, UnixName},
        unix_ns::lookup,
    };
    let a = crate::protocol_support::make_socket(1);
    let b = crate::protocol_support::make_socket(1);
    let aname = UnixName::Path(b"/tmp/byte-\xff".to_vec());
    let bname = UnixName::Path(b"/tmp/byte-\xfe".to_vec());
    a.ops().bind(&a, &SockAddr::Unix(aname.clone())).unwrap();
    b.ops().bind(&b, &SockAddr::Unix(bname.clone())).unwrap();
    assert!(Arc::ptr_eq(&lookup(&aname).unwrap(), &a));
    assert!(Arc::ptr_eq(&lookup(&bname).unwrap(), &b));
}
