//! Handle shim for namespace disk tests. There are no open files, mappings or
//! asynchronous finalizers here; this does not model kernel inode lifetimes.
use super::{Ext2Fs, LiveInode};

pub struct FileHandle {
    pub(super) ino: u32,
    pub object: LiveInode,
}

impl FileHandle {
    pub fn verify(&self, fs: &Ext2Fs) -> Result<u32, &'static str> {
        fs.read_inode(self.ino)?;
        Ok(self.ino)
    }

    pub fn release_sole(self) -> Result<LiveInode, Self> {
        Ok(self.object)
    }
}
