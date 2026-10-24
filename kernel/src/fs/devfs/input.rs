//! The input directory; event endpoints themselves have independent descriptions.
use crate::{
    ipc::object::IpcFile,
    syscall::{errno::*, fs::Stat, userptr},
};

pub const INODE: u64 = super::entries::INPUT_DIRECTORY_INO;
#[derive(Debug, Default)]
pub struct Directory {
    position: spin::Mutex<usize>,
}
impl IpcFile for Directory {
    fn as_any(&self) -> &dyn core::any::Any {
        self
    }
    fn fill_stat(&self, st: &mut Stat) {
        st.st_dev = 0xffff0002;
        st.st_ino = INODE;
        st.st_mode = 0o040555;
        st.st_nlink = 2;
        st.st_blksize = 4096;
    }
    fn read(&self, _buf: u64, _count: usize) -> Result<u64, u64> {
        Err(EISDIR as u64)
    }
    fn getdents(&self, dirp: u64, count: usize) -> Result<u64, u64> {
        let entries = [(INODE, 4u8, "."), (1, 4, ".."), (40, 2, "events")];
        let mut position = self.position.lock();
        let mut bytes = alloc::vec::Vec::new();
        let mut next = *position;
        for &(ino, kind, name) in entries.iter().skip(*position) {
            let size = (19 + name.len() + 1 + 7) & !7;
            if bytes.len() + size > count {
                break;
            }
            next += 1;
            bytes.extend_from_slice(&ino.to_le_bytes());
            bytes.extend_from_slice(&(next as i64).to_le_bytes());
            bytes.extend_from_slice(&(size as u16).to_le_bytes());
            bytes.push(kind);
            bytes.extend_from_slice(name.as_bytes());
            bytes.resize(bytes.len() + size - 19 - name.len(), 0);
        }
        if next == *position && next < entries.len() {
            return Err(EINVAL as u64);
        }
        userptr::write_user_bytes(dirp, bytes.as_ptr(), bytes.len())?;
        *position = next;
        Ok(bytes.len() as u64)
    }
    fn seek(&self, offset: i64, whence: i32) -> Result<u64, u64> {
        if whence != 0 || !(0..=3).contains(&offset) {
            return Err(EINVAL as u64);
        }
        *self.position.lock() = offset as usize;
        Ok(offset as u64)
    }
}
