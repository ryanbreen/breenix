//! On-disk fault geometry and a private, uncached read-only image.
use super::Ext2Fs;
use crate::fs::attrs::InodeAttrs;
use crate::fs::vfs::fault::{FaultBlockDevice, FaultImage, FaultLayout};
use crate::{
    block::BlockDevice,
    fs::{errno::Errno, vfs::*},
    syscall::errno::*,
};
use alloc::{
    boxed::Box,
    sync::{Arc, Weak},
    vec::Vec,
};

/// The ext2 inode record fields this module rewrites, by byte offset within the
/// on-disk inode. Sourced from the ext2 on-disk format, matching
/// `super::Ext2Inode`'s field order.
const INODE_OFF_MODE: usize = 0;
const INODE_OFF_SIZE: usize = 4;
const INODE_OFF_LINKS: usize = 26;
const INODE_OFF_BLOCK_PTRS: usize = 40;
const INODE_BLOCK_PTR_COUNT: usize = 15;
/// Smallest on-disk inode record; the corruption bounds check uses this size.
const INODE_MIN_RECORD: usize = 128;

fn corrupt_inode(buf: &mut [u8], offset: usize, which: u64) -> bool {
    if offset + INODE_MIN_RECORD > buf.len() {
        // Cannot corrupt what does not fit; leave the buffer honest
        // and let the leg report a missing fault rather than a
        // half-applied one.
        return false;
    }
    let record = &mut buf[offset..offset + INODE_MIN_RECORD];
    match which {
        0 => {
            // A regular file, one link, and a size larger than any
            // filesystem this kernel can mount.
            record[INODE_OFF_MODE..INODE_OFF_MODE + 2].copy_from_slice(&0x81FFu16.to_le_bytes());
            record[INODE_OFF_LINKS..INODE_OFF_LINKS + 2].copy_from_slice(&1u16.to_le_bytes());
            record[INODE_OFF_SIZE..INODE_OFF_SIZE + 4]
                .copy_from_slice(&0xFFFF_FFF0u32.to_le_bytes());
        }
        _ => {
            // A plausible small regular file whose 15 block
            // pointer names a block the device does not have.
            record[INODE_OFF_MODE..INODE_OFF_MODE + 2].copy_from_slice(&0x81FFu16.to_le_bytes());
            record[INODE_OFF_LINKS..INODE_OFF_LINKS + 2].copy_from_slice(&1u16.to_le_bytes());
            record[INODE_OFF_SIZE..INODE_OFF_SIZE + 4].copy_from_slice(&1024u32.to_le_bytes());
            for i in 0..INODE_BLOCK_PTR_COUNT {
                let at = INODE_OFF_BLOCK_PTRS + i * 4;
                record[at..at + 4].copy_from_slice(&0xDEAD_BEEFu32.to_le_bytes());
            }
        }
    }
    true
}

/// Sector arithmetic for one inode record, mirroring `Ext2Inode::read_from`.
///
/// Returns `(device_sector, byte_offset_within_that_sector)`.
fn inode_record_location(
    fs: &super::Ext2Fs,
    inode_num: u64,
    device_block_size: usize,
) -> (u64, u64) {
    let sb = &fs.superblock;
    let inodes_per_group =
        unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(sb.s_inodes_per_group)) };
    let s_rev_level = unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(sb.s_rev_level)) };
    let s_inode_size = unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(sb.s_inode_size)) };
    let s_log_block_size =
        unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(sb.s_log_block_size)) };

    let inode_index = inode_num - 1;
    let group = (inode_index / u64::from(inodes_per_group)) as usize;
    let local_index = inode_index % u64::from(inodes_per_group);
    let inode_table_block = unsafe {
        core::ptr::read_unaligned(core::ptr::addr_of!(fs.block_groups[group].bg_inode_table))
    };
    let inode_size: u32 = if s_rev_level == 0 {
        128
    } else {
        s_inode_size as u32
    };
    let ext2_block_size = 1024u32 << s_log_block_size;
    let byte_offset = local_index * u64::from(inode_size);
    let target_ext2_block = u64::from(inode_table_block) + byte_offset / u64::from(ext2_block_size);
    let offset_in_ext2_block = byte_offset % u64::from(ext2_block_size);

    let sectors_per_ext2_block = (ext2_block_size as usize / device_block_size) as u64;
    let sector = target_ext2_block * sectors_per_ext2_block
        + (offset_in_ext2_block as usize / device_block_size) as u64;
    let offset_in_sector = (offset_in_ext2_block as usize % device_block_size) as u64;
    (sector, offset_in_sector)
}

pub(super) fn mount(device: Box<dyn BlockDevice>) -> Result<FaultImage, Errno> {
    let block_size = device.block_size();
    let fs = Ext2Fs::new_uncached(
        Box::new(FaultBlockDevice::new(device, corrupt_inode)),
        usize::MAX,
    )
    .map_err(super::errno::errno)?;
    let inode = fs
        .read_inode(super::EXT2_ROOT_INO)
        .map_err(super::errno::errno)?;
    let sectors = (fs.superblock.block_size() / block_size) as u64;
    let blocks = unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(inode.i_block)) };
    let (inode_sector, inode_offset) =
        inode_record_location(&fs, super::EXT2_ROOT_INO.into(), block_size);
    let layout = FaultLayout {
        header_sector: (1024 / block_size) as u64,
        data_sector: blocks[0] as u64 * sectors,
        data_sectors: sectors.max(1),
        inode_sector,
        inode_offset,
    };
    let filesystem: Arc<Shadow> = Arc::new_cyclic(|me| Shadow {
        fs: spin::RwLock::new(fs),
        me: me.clone(),
    });
    Ok(FaultImage { filesystem, layout })
}

struct Shadow {
    fs: spin::RwLock<Ext2Fs>,
    me: Weak<Shadow>,
}
impl FileSystem for Shadow {
    fn kind(&self) -> Result<&'static str, Errno> {
        Ok("ext2")
    }
    fn root(&self) -> Result<u64, Errno> {
        Ok(super::EXT2_ROOT_INO.into())
    }
    fn statfs(&self) -> Result<StatFs, Errno> {
        Err(Errno(EOPNOTSUPP))
    }
    fn sync(&self) -> Result<(), Errno> {
        Err(Errno(EOPNOTSUPP))
    }
    fn check_live(&self) -> Result<(), Errno> {
        let fs = self.fs.read();
        fs.read_inode(super::EXT2_ROOT_INO)
            .map_err(super::errno::errno)?;
        Ok(())
    }
    fn read_only(&self) -> Result<bool, Errno> {
        Ok(true)
    }
    fn max_file_size(&self) -> Result<u64, Errno> {
        Ok(self.fs.read().max_file_size())
    }
    fn getattr(&self, ino: u64) -> Result<NodeAttrs, Errno> {
        let fs = self.fs.read();
        super::adapter::attributes(
            ino,
            &fs.read_inode(super::ino32(ino)?)
                .map_err(super::errno::errno)?,
        )
    }
    fn setattr(&self, ino: u64, attrs: SetAttr) -> Result<(), Errno> {
        let _ = (ino, attrs);
        Err(Errno(EOPNOTSUPP))
    }
    fn open(&self, ino: u64) -> Result<Arc<VNode>, Errno> {
        let attrs = self.getattr(ino)?;
        let filesystem = self.me.upgrade().ok_or(Errno(EIO))?;
        Ok(Arc::new(VNode::new(
            usize::MAX,
            ino,
            0,
            attrs.size,
            attrs.times,
            filesystem,
            0,
            Arc::new(()),
        )?))
    }
    fn lookup(&self, dir: u64, name: &[u8]) -> Result<u64, Errno> {
        let _ = (dir, name);
        Err(Errno(EOPNOTSUPP))
    }
    fn lookup_open(&self, dir: &VNode, name: &[u8]) -> Result<Arc<VNode>, Errno> {
        let _ = (dir, name);
        Err(Errno(EOPNOTSUPP))
    }
    fn touch(&self, node: &VNode, read: bool) {
        let _ = (node, read);
    }
    fn parent(&self, dir: u64) -> Result<u64, Errno> {
        let _ = (dir,);
        Err(Errno(EOPNOTSUPP))
    }
    fn readdir(
        &self,
        dir: u64,
        cookie: u64,
        sink: &mut dyn FnMut(DirEnt, u64) -> Result<bool, Errno>,
    ) -> Result<u64, Errno> {
        let _ = (dir, cookie, sink);
        Err(Errno(EOPNOTSUPP))
    }
    fn create(&self, dir: u64, name: &[u8], node: NewNode<'_>) -> Result<u64, Errno> {
        let _ = (dir, name, node);
        Err(Errno(EOPNOTSUPP))
    }
    fn create_open(
        &self,
        dir: u64,
        name: &[u8],
        kind: NodeKind,
        check: &dyn Fn(NodeAttrs) -> Result<InodeAttrs, Errno>,
        prepare: &mut dyn FnMut(Arc<VNode>) -> Result<(), Errno>,
    ) -> Result<(), Errno> {
        let _ = (dir, name, kind, check, prepare);
        Err(Errno(EOPNOTSUPP))
    }
    fn link(&self, ino: u64, dir: u64, name: &[u8], check: &LinkCheck<'_>) -> Result<(), Errno> {
        let _ = (ino, dir, name, check);
        Err(Errno(EOPNOTSUPP))
    }
    fn remove(&self, dir: u64, name: &[u8], which: Remove) -> Result<(), Errno> {
        let _ = (dir, name, which);
        Err(Errno(EOPNOTSUPP))
    }
    fn rename(
        &self,
        from_dir: u64,
        from: &[u8],
        to_dir: u64,
        to: &[u8],
        mode: RenameMode,
        check: &RenameCheck<'_>,
    ) -> Result<(), Errno> {
        let _ = (from_dir, from, to_dir, to, mode, check);
        Err(Errno(EOPNOTSUPP))
    }
    fn readlink(&self, ino: u64) -> Result<Vec<u8>, Errno> {
        let _ = (ino,);
        Err(Errno(EOPNOTSUPP))
    }
    fn read(&self, node: &VNode, off: u64, buf: &mut [u8]) -> Result<usize, Errno> {
        let fs = self.fs.read();
        let inode = fs
            .read_inode(super::ino32(node.ino)?)
            .map_err(super::errno::errno)?;
        let bytes = if inode.is_dir() {
            fs.read_directory(&inode).map_err(super::errno::errno)
        } else {
            super::read_file(fs.device.as_ref(), &inode, &fs.superblock).map_err(|_| Errno(EIO))
        }?;
        let start = core::cmp::min(off, bytes.len() as u64) as usize;
        let count = core::cmp::min(buf.len(), bytes.len() - start);
        buf[..count].copy_from_slice(&bytes[start..start + count]);
        Ok(count)
    }
}
