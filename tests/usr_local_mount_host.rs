// Storage fixture for the live layer. The Python entry point compiles the
// production cutover function and inode/mount table types into this harness.
use std::{cell::RefCell, collections::BTreeMap, sync::{Arc, Mutex}};

extern crate alloc;
use block::BlockDevice;
use fs::{attrs::InodeAttrs, errno::Errno, vfs::{FileSystem, NewNode, NodeKind}};
use syscall::errno::*;

mod fs {
    pub use crate::{attrs, errno};
    pub mod vfs {
        use super::{attrs::InodeAttrs, errno::Errno};
        use std::sync::Arc;

        pub trait FileSystem {
            fn root(&self) -> Result<u64, Errno>;
            fn lookup(&self, parent: u64, name: &[u8]) -> Result<u64, Errno>;
            fn getattr(&self, ino: u64) -> Result<NodeAttrs, Errno>;
            fn open(&self, ino: u64) -> Result<Arc<dyn Send + Sync>, Errno>;
            fn create(&self, parent: u64, name: &[u8], node: NewNode) -> Result<u64, Errno>;
            fn sync(&self) -> Result<(), Errno>;
        }

        #[derive(PartialEq, Eq)]
        pub enum NodeKind { Directory }
        pub struct NodeAttrs { pub kind: NodeKind, pub attrs: InodeAttrs }
        pub enum NewNode { Directory { attrs: InodeAttrs } }
    }
    pub mod breenfs {
        pub mod vfs { pub type BreenFs = crate::Tree; }
    }
}

mod syscall {
    pub use crate::syscall_errno as errno;
}

mod block {
    pub trait BlockDevice {}
    pub mod cache_kernel { pub fn system_cache() {} }
}

struct Disk;
impl block::BlockDevice for Disk {}

#[derive(Default)]
struct Tree(Mutex<Vec<Directory>>);
struct Directory { children: BTreeMap<Vec<u8>, u64>, attrs: InodeAttrs }

impl Tree {
    fn new() -> Self {
        Self(Mutex::new(vec![Directory {
            children: BTreeMap::new(),
            attrs: InodeAttrs::new(0o40755, 0, 0),
        }]))
    }

    fn mkdir(&self, parent: u64, name: &[u8]) -> u64 {
        self.create(parent, name, NewNode::Directory { attrs: InodeAttrs::new(0o755, 0, 0) }).unwrap()
    }

    fn mount_writable_cached(_: Arc<dyn block::BlockDevice>, _: (), _: usize, _: u64)
        -> Result<Arc<Self>, Errno> {
        let writer = Self::new();
        writer.mkdir(0, b"root");
        let shared = writer.mkdir(0, b".breenix-shared");
        for name in [b"etc", b"var", b"opt", b"srv", b"tmp", b"run"] {
            writer.mkdir(shared, name);
        }
        Ok(Arc::new(writer))
    }

    fn generation_view(&self, _: usize, _: &[u8]) -> Result<Arc<dyn FileSystem>, Errno> {
        let view = Self::new();
        for name in [b"root".as_slice(), b"etc", b"var", b"opt", b"srv", b"tmp", b"run"] {
            view.mkdir(0, name);
        }
        let usr = view.mkdir(0, b"usr");
        view.mkdir(usr, b"local");
        Ok(Arc::new(view))
    }
}

impl FileSystem for Tree {
    fn root(&self) -> Result<u64, Errno> { Ok(0) }

    fn lookup(&self, parent: u64, name: &[u8]) -> Result<u64, Errno> {
        self.0.lock().unwrap()[parent as usize].children.get(name).copied().ok_or(Errno(ENOENT))
    }

    fn getattr(&self, ino: u64) -> Result<fs::vfs::NodeAttrs, Errno> {
        let attrs = self.0.lock().unwrap()[ino as usize].attrs;
        Ok(fs::vfs::NodeAttrs { kind: NodeKind::Directory, attrs })
    }

    fn open(&self, _: u64) -> Result<Arc<dyn Send + Sync>, Errno> { Ok(Arc::new(())) }

    fn create(&self, parent: u64, name: &[u8], NewNode::Directory { mut attrs }: NewNode)
        -> Result<u64, Errno> {
        let mut tree = self.0.lock().unwrap();
        let id = tree.len() as u64;
        assert!(tree[parent as usize].children.insert(name.to_vec(), id).is_none());
        attrs.mode |= 0o40000;
        tree.push(Directory { children: BTreeMap::new(), attrs });
        Ok(id)
    }

    fn sync(&self) -> Result<(), Errno> { Ok(()) }
}

mod registry {
    use super::{mount_table, *};

    pub enum Backend { Fs(Arc<dyn FileSystem>) }
    pub type MountPoint = mount_table::Entry<Backend>;

    thread_local! {
        pub static TABLE: RefCell<mount_table::MountTable<Backend>> =
            const { RefCell::new(mount_table::MountTable::new()) };
    }

    pub fn allocate_id() -> Result<usize, Errno> {
        TABLE.with(|table| table.borrow_mut().allocate_id())
    }

    pub fn register_cutover(entries: Vec<MountPoint>, writer: usize) -> Result<(), Errno> {
        TABLE.with(|table| {
            for mut entry in entries {
                entry.device_id = writer as u64;
                table.borrow_mut().insert(entry)?;
            }
            Ok(())
        })
    }
}

struct ExplicitMount<'a> { id: usize, path: &'a [u8] }

impl ExplicitMount<'_> {
    fn entry(&self, fs: Arc<dyn FileSystem>) -> Result<registry::MountPoint, Errno> {
        Ok(registry::MountPoint::new(
            self.id, self.path.to_vec(), "breenfs",
            attrs::MountFlags { read_only: true, ..Default::default() },
            registry::Backend::Fs(fs.clone()), fs.root()?, None, None,
        ))
    }
}

fn main() {
    let root = registry::allocate_id().unwrap();
    let view = mount_cutover(&ExplicitMount { id: root, path: b"/" }, Arc::new(Disk), "test").unwrap();
    let usr = view.lookup(view.root().unwrap(), b"usr").unwrap();
    let cover = view.lookup(usr, b"local").unwrap();

    let mode = std::env::args().nth(2).unwrap();
    registry::TABLE.with(|table| {
        let table = table.borrow();
        let id = table.find_mount(b"/usr/local").expect("missing /usr/local mount");
        let entry = table.get(id).unwrap();
        let registry::Backend::Fs(writer) = &entry.backend;
        if mode == "mount" {
            assert!(table.get(root).unwrap().flags.read_only);
            assert!(!entry.flags.read_only);
            assert_eq!(entry.kind, "breenfs");
            assert_eq!(entry.cover, Some((root, cover)));
            assert_eq!(entry.parent, Some((root, usr)));
            assert_eq!(table.enter((root, cover)), Some((id, entry.root)));
            let opt = table.get(table.find_mount(b"/opt").unwrap()).unwrap();
            assert_eq!(entry.device_id, opt.device_id);
            let shared = writer.lookup(writer.root().unwrap(), b".breenix-shared").unwrap();
            let source = writer.lookup(shared, b"usr-local").unwrap();
            assert_eq!(entry.bind, Some((opt.bind.unwrap().0, source)));
        } else {
            assert_eq!(mode, "modes");
            for name in [b"bin".as_slice(), b"sbin", b"lib"] {
                let inode = writer.lookup(entry.root, name).unwrap();
                let attrs = writer.getattr(inode).unwrap().attrs;
                assert!(attrs.is_dir());
                assert_eq!((attrs.uid, attrs.gid, attrs.permissions()), (0, 10, 0o2775));
            }
        }
    });
}
