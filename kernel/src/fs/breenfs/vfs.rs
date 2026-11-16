//! Read-only breenfs VFS adapter (stub for the read conformance check).
use crate::{
    block::{
        cache::{BufferCache, Wait},
        BlockDevice,
    },
    fs::{attrs::InodeAttrs, errno::Errno, vfs::*},
    syscall::errno::EOPNOTSUPP,
};
use alloc::{sync::Arc, vec::Vec};
pub struct BreenFs;
impl BreenFs {
    pub fn mount<W: Wait + 'static>(
        _: Arc<dyn BlockDevice>,
        _: Arc<BufferCache<W>>,
        _: usize,
        _: u64,
    ) -> Result<Arc<Self>, Errno> {
        Ok(Arc::new(Self))
    }
}
impl FileSystem for BreenFs {
    fn kind(&self) -> Result<&'static str, Errno> {
        Err(Errno(EOPNOTSUPP))
    }
    fn root(&self) -> Result<u64, Errno> {
        Err(Errno(EOPNOTSUPP))
    }
    fn statfs(&self) -> Result<StatFs, Errno> {
        Err(Errno(EOPNOTSUPP))
    }
    fn sync(&self) -> Result<(), Errno> {
        Err(Errno(EOPNOTSUPP))
    }
    fn read_only(&self) -> Result<bool, Errno> {
        Err(Errno(EOPNOTSUPP))
    }
    fn max_file_size(&self) -> Result<u64, Errno> {
        Err(Errno(EOPNOTSUPP))
    }
    fn getattr(&self, _: u64) -> Result<NodeAttrs, Errno> {
        Err(Errno(EOPNOTSUPP))
    }
    fn setattr(&self, _: u64, _: SetAttr) -> Result<(), Errno> {
        Err(Errno(EOPNOTSUPP))
    }
    fn open(&self, _: u64) -> Result<Arc<VNode>, Errno> {
        Err(Errno(EOPNOTSUPP))
    }
    fn lookup(&self, _: u64, _: &[u8]) -> Result<u64, Errno> {
        Err(Errno(EOPNOTSUPP))
    }
    fn lookup_open(&self, _: &VNode, _: &[u8]) -> Result<Arc<VNode>, Errno> {
        Err(Errno(EOPNOTSUPP))
    }
    fn touch(&self, _: &VNode, _: bool) {}
    fn parent(&self, _: u64) -> Result<u64, Errno> {
        Err(Errno(EOPNOTSUPP))
    }
    fn readdir(
        &self,
        _: u64,
        _: u64,
        _: &mut dyn FnMut(DirEnt, u64) -> Result<bool, Errno>,
    ) -> Result<u64, Errno> {
        Err(Errno(EOPNOTSUPP))
    }
    fn create(&self, _: u64, _: &[u8], _: NewNode<'_>) -> Result<u64, Errno> {
        Err(Errno(EOPNOTSUPP))
    }
    fn create_open(
        &self,
        _: u64,
        _: &[u8],
        _: NodeKind,
        _: &dyn Fn(NodeAttrs) -> Result<InodeAttrs, Errno>,
        _: &mut dyn FnMut(Arc<VNode>) -> Result<(), Errno>,
    ) -> Result<(), Errno> {
        Err(Errno(EOPNOTSUPP))
    }
    fn link(&self, _: u64, _: u64, _: &[u8], _: &LinkCheck<'_>) -> Result<(), Errno> {
        Err(Errno(EOPNOTSUPP))
    }
    fn remove(&self, _: u64, _: &[u8], _: Remove) -> Result<(), Errno> {
        Err(Errno(EOPNOTSUPP))
    }
    fn rename(
        &self,
        _: u64,
        _: &[u8],
        _: u64,
        _: &[u8],
        _: RenameMode,
        _: &RenameCheck<'_>,
    ) -> Result<(), Errno> {
        Err(Errno(EOPNOTSUPP))
    }
    fn readlink(&self, _: u64) -> Result<Vec<u8>, Errno> {
        Err(Errno(EOPNOTSUPP))
    }
}
