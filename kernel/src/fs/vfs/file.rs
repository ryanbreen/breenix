//! Backend-neutral state retained by an open file. Descriptor positions stay
//! with the caller, so duplicated descriptors can share their position lock.
use super::{DirEnt, FileSystem, NodeKind, VNode};
use crate::fs::errno::Errno;
use alloc::sync::Arc;

pub struct OpenFile {
    pub fs: Arc<dyn FileSystem>,
    pub node: Arc<VNode>,
}

impl OpenFile {
    /// Return the position after the last entry the sink accepted. Positions 0
    /// and 1 name the VFS dots; backend cookies start at 2 (cowfs DIR_INDEX).
    pub fn dirents(
        &self,
        mut pos: u64,
        emit: &mut dyn FnMut(DirEnt) -> Result<bool, Errno>,
    ) -> Result<u64, Errno> {
        if pos < 2 {
            // The backend validates that this is a directory before dots escape.
            let parent = self.fs.parent(self.node.ino)?;
            for (position, ino, name) in [
                (0, self.node.ino, b".".as_slice()),
                (1, parent, b"..".as_slice()),
            ] {
                if pos > position {
                    continue;
                }
                if !emit(DirEnt {
                    ino,
                    kind: NodeKind::Directory,
                    name: name.to_vec(),
                })? {
                    return Ok(pos);
                }
                pos = position + 1;
            }
        }
        self.fs.readdir(self.node.ino, pos.max(2), emit)
    }
}
