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
    sectors_per_block: u64,
}
impl<'a, B: BlockDevice + ?Sized> Extension<'a, B> {
    pub(super) fn new(
        device: &'a B,
        sb: &Ext2Superblock,
        groups: &[Ext2BlockGroupDesc],
    ) -> Result<Self, &'static str> {
        let unit = device.block_size();
        if unit == 0
            || unit > 4096
            || unit % 4 != 0
            || sb.block_size() < unit
            || sb.block_size() % unit != 0
        {
            return Err("Invalid directory block geometry");
        }
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
            sectors_per_block: scale,
        })
    }
    fn bitmap(&self, sector: u64) -> bool {
        self.bitmaps
            .iter()
            .any(|&(start, end)| sector >= start && sector < end)
    }
    /// Resolve this append's addressing chain from staged bytes, in child-first order.
    /// i_block has twelve direct pointers followed by single/double/triple roots.
    pub(super) fn block_order(
        &self,
        addresses: [u32; 15],
        logical: u32,
        data: u32,
    ) -> Result<[u32; 4], BlockError> {
        let mut order = [data, 0, 0, 0];
        if logical < 12 {
            return if addresses[logical as usize] == data {
                Ok(order)
            } else {
                Err(BlockError::IoError)
            };
        }
        let pointers = (self.sectors_per_block * self.block_size() as u64 / 4) as u32;
        if pointers == 0 {
            return Err(BlockError::IoError);
        }
        let mut relative = logical - 12;
        let (root, depth) = if relative < pointers {
            (12, 1)
        } else {
            relative -= pointers;
            if relative < pointers * pointers {
                (13, 2)
            } else {
                relative -= pointers * pointers;
                (14, 3)
            }
        };
        let mut block = addresses[root];
        let mut divisor = pointers.pow(depth - 1);
        let mut sector = [0u8; 4096];
        for level in 0..depth as usize {
            if block == 0 || self.block_size() > sector.len() {
                return Err(BlockError::IoError);
            }
            order[depth as usize - level] = block;
            let index = relative / divisor;
            if index >= pointers {
                return Err(BlockError::IoError);
            }
            relative %= divisor;
            let offset = index as usize * 4;
            let number =
                block as u64 * self.sectors_per_block + (offset / self.block_size()) as u64;
            self.read_block(number, &mut sector[..self.block_size()])?;
            let at = offset % self.block_size();
            block = u32::from_le_bytes(
                sector[at..at + 4]
                    .try_into()
                    .map_err(|_| BlockError::IoError)?,
            );
            divisor = (divisor / pointers).max(1);
        }
        if block != data {
            return Err(BlockError::IoError);
        }
        Ok(order)
    }

    pub(super) fn publish(&self, blocks: &[u32]) -> Result<(), bool> {
        let mut sectors = self.sectors.lock();
        // Staged initialization writes may later be replaced by pointer writes.
        // First-seen sector order is therefore insufficient for safe publication.
        sectors.sort_unstable_by_key(|(sector, _, _)| {
            let rank = if self.bitmap(*sector) {
                0
            } else {
                blocks
                    .iter()
                    .position(|&block| {
                        block != 0 && *sector / self.sectors_per_block == block as u64
                    })
                    .map_or(blocks.len() + 1, |index| index + 1)
            };
            (rank, *sector)
        });
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
    fn is_read_only(&self) -> bool { self.device.is_read_only() }

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
