use super::{Ext2BlockGroupDesc, Ext2Superblock};
use crate::block::{BlockDevice, BlockError};
use alloc::vec::Vec;

// Stage a directory extension, including allocation bitmaps and inode sectors.
// No disk bytes change before the whole plan is prepared. On publish failure,
// restore references first, then allocation bits; never recycle a block while
// an uncertain pointer write could still refer to it.
pub(super) struct Extension<'a, B: BlockDevice + ?Sized> {
    device: &'a B,
    sectors: spin::Mutex<Vec<(u64, Vec<u8>, Vec<u8>)>>,
    bitmaps: Vec<(u64, u64)>,
}
impl<'a, B: BlockDevice + ?Sized> Extension<'a, B> {
    pub(super) fn new(
        device: &'a B,
        sb: &Ext2Superblock,
        groups: &[Ext2BlockGroupDesc],
    ) -> Result<Self, &'static str> {
        let mut bitmaps = Vec::new();
        bitmaps
            .try_reserve_exact(groups.len())
            .map_err(|_| "No memory for directory transaction")?;
        let scale = sb.block_size() as u64 / device.block_size() as u64;
        for group in groups {
            let start = group.bg_block_bitmap as u64 * scale;
            bitmaps.push((start, start + scale));
        }
        Ok(Self {
            device,
            sectors: spin::Mutex::new(Vec::new()),
            bitmaps,
        })
    }
    fn bitmap(&self, sector: u64) -> bool {
        self.bitmaps
            .iter()
            .any(|&(start, end)| sector >= start && sector < end)
    }
    pub(super) fn publish(&self) -> Result<(), bool> {
        let sectors = self.sectors.lock();
        let mut attempted = 0;
        for (sector, _, new) in sectors.iter() {
            attempted += 1;
            if self.device.write_block(*sector, new).is_err() {
                let mut restored = true;
                for (number, old, _) in sectors[..attempted]
                    .iter()
                    .rev()
                    .filter(|(n, _, _)| !self.bitmap(*n))
                {
                    if self.device.write_block(*number, old).is_err() {
                        restored = false;
                    }
                }
                // A failed restore retains allocation bits, even if that leaks space:
                // corruption would be worse. Successful restores reclaim all blocks.
                if restored {
                    for (number, old, _) in sectors[..attempted]
                        .iter()
                        .rev()
                        .filter(|(n, _, _)| self.bitmap(*n))
                    {
                        if self.device.write_block(*number, old).is_err() {
                            restored = false;
                        }
                    }
                }
                return Err(restored);
            }
        }
        Ok(())
    }
}
impl<B: BlockDevice + ?Sized> BlockDevice for Extension<'_, B> {
    fn identity(&self) -> usize {
        self.device.identity()
    }
    fn block_size(&self) -> usize {
        self.device.block_size()
    }
    fn num_blocks(&self) -> u64 {
        self.device.num_blocks()
    }
    fn flush(&self) -> Result<(), BlockError> {
        self.device.flush()
    }
    fn read_block(&self, sector: u64, bytes: &mut [u8]) -> Result<(), BlockError> {
        let sectors = self.sectors.lock();
        if let Some((_, _, new)) = sectors.iter().find(|(n, _, _)| *n == sector) {
            bytes[..new.len()].copy_from_slice(new);
            Ok(())
        } else {
            self.device.read_block(sector, bytes)
        }
    }
    fn write_block(&self, sector: u64, bytes: &[u8]) -> Result<(), BlockError> {
        let size = self.block_size();
        if bytes.len() < size {
            return Err(BlockError::IoError);
        }
        let mut sectors = self.sectors.lock();
        if let Some((_, _, new)) = sectors.iter_mut().find(|(n, _, _)| *n == sector) {
            new.copy_from_slice(&bytes[..size]);
            return Ok(());
        }
        sectors
            .try_reserve(1)
            .map_err(|_| BlockError::OutOfMemory)?;
        let mut old = Vec::new();
        let mut new = Vec::new();
        old.try_reserve_exact(size)
            .map_err(|_| BlockError::OutOfMemory)?;
        new.try_reserve_exact(size)
            .map_err(|_| BlockError::OutOfMemory)?;
        old.resize(size, 0);
        self.device.read_block(sector, &mut old)?;
        new.extend_from_slice(&bytes[..size]);
        sectors.push((sector, old, new));
        Ok(())
    }
}
