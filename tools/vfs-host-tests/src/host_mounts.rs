//! Host-only mount guards. The disk shim has no detached transfers to wait for.
use crate::{
    block,
    storage::{self, Ext2BlockGroupDesc, Ext2Fs, Ext2Inode, Ext2Superblock},
};
use std::{
    collections::BTreeMap,
    ops::{Deref, DerefMut},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, MutexGuard,
    },
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MountPin {
    pub mount_id: usize,
    pub instance: u64,
}
static MOUNTS: Mutex<BTreeMap<usize, Option<Ext2Fs>>> = Mutex::new(BTreeMap::new());
static NEXT: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(1);

pub struct Guard {
    mounts: MutexGuard<'static, BTreeMap<usize, Option<Ext2Fs>>>,
    id: usize,
}
impl Deref for Guard {
    type Target = Option<Ext2Fs>;
    fn deref(&self) -> &Self::Target {
        &self.mounts[&self.id]
    }
}
impl DerefMut for Guard {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.mounts.get_mut(&self.id).unwrap()
    }
}
impl Guard {
    pub fn wait_for_inode(self, _ino: u32) -> Result<Self, &'static str> {
        Ok(self)
    }
    pub fn wait_for_entry_data(
        self,
        _parent: &storage::live_inode::FileHandle,
        _name: &[u8],
    ) -> Result<Self, &'static str> {
        Ok(self)
    }
}
pub fn read_mount(pin: MountPin) -> Result<Guard, &'static str> {
    let mounts = MOUNTS.lock().unwrap();
    if !mounts.contains_key(&pin.mount_id) {
        return Err("Unknown ext2 mount");
    }
    Ok(Guard {
        mounts,
        id: pin.mount_id,
    })
}
pub fn write_mount(pin: MountPin) -> Result<Guard, &'static str> {
    read_mount(pin)
}

struct Disk(Mutex<Vec<u8>>, Arc<AtomicBool>);
impl block::BlockDevice for Disk {
    fn is_read_only(&self) -> bool {
        self.1.load(Ordering::Acquire)
    }
    fn read_block(&self, sector: u64, out: &mut [u8]) -> Result<(), block::BlockError> {
        let start = sector as usize * 512;
        let bytes = self.0.lock().unwrap();
        let data = bytes
            .get(start..start + out.len())
            .ok_or(block::BlockError::OutOfBounds)?;
        out.copy_from_slice(data);
        Ok(())
    }
    fn write_block(&self, sector: u64, input: &[u8]) -> Result<(), block::BlockError> {
        if self.is_read_only() {
            return Err(block::BlockError::IoError);
        }
        let start = sector as usize * 512;
        let mut bytes = self.0.lock().unwrap();
        let data = bytes
            .get_mut(start..start + input.len())
            .ok_or(block::BlockError::OutOfBounds)?;
        data.copy_from_slice(input);
        Ok(())
    }
    fn block_size(&self) -> usize {
        512
    }
    fn num_blocks(&self) -> u64 {
        4096
    }
    fn flush(&self) -> Result<(), block::BlockError> {
        Ok(())
    }
}

pub struct Fixture(MountPin, Arc<AtomicBool>);
impl Fixture {
    pub fn new() -> Self {
        let mut bytes = vec![0; 512 * 4096];
        bytes[3 * 4096..3 * 4096 + 2].fill(0xff);
        bytes[3 * 4096 + 2] = 0x3f;
        bytes[4 * 4096] = 3;
        bytes[21 * 4096 + 4..21 * 4096 + 6].copy_from_slice(&4096u16.to_le_bytes());
        let mut sb = Ext2Superblock::from_bytes(&[0; 1024]).unwrap();
        sb.s_blocks_count = 512;
        sb.s_free_blocks_count = 490;
        sb.s_inodes_count = 128;
        sb.s_free_inodes_count = 126;
        sb.s_log_block_size = 2;
        sb.s_blocks_per_group = 512;
        sb.s_inodes_per_group = 128;
        let mut group: Ext2BlockGroupDesc = unsafe { core::mem::zeroed() };
        group.bg_inode_table = 8;
        group.bg_block_bitmap = 3;
        group.bg_inode_bitmap = 4;
        group.bg_free_blocks_count = 490;
        group.bg_free_inodes_count = 126;
        let read_only = Arc::new(AtomicBool::new(false));
        let mut fs = Ext2Fs::host_new(
            Box::new(Disk(Mutex::new(bytes), read_only.clone())),
            sb,
            vec![group],
        );
        let mut root = Ext2Inode::new_directory(0o755);
        root.i_size = 4096;
        root.i_blocks = 8;
        root.i_block[0] = 21;
        fs.write_inode(2, &root).unwrap();
        fs.superblock.write_to(fs.device.as_ref()).unwrap();
        Ext2BlockGroupDesc::write_table(fs.device.as_ref(), &fs.superblock, &fs.block_groups)
            .unwrap();
        let id = NEXT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        MOUNTS.lock().unwrap().insert(id, Some(fs));
        Self(
            MountPin {
                mount_id: id,
                instance: id as u64,
            },
            read_only,
        )
    }
    pub fn remount_read_only(&self) {
        self.1.store(true, Ordering::Release);
    }
    pub fn pin(&self) -> MountPin {
        self.0
    }
}
impl Default for Fixture {
    fn default() -> Self {
        Self::new()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        MOUNTS.lock().unwrap().remove(&self.0.mount_id);
    }
}
