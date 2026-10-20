//! One GPT partition of a hardware disk as a block device: LBA 0 is the partition's first sector, and a request
//! past its last sector is refused before it reaches the disk (docs/metal/README.md, "Boot intents and root
//! selection"). The write fence (`write_policy`) still checks each write the driver receives.

use alloc::boxed::Box;

use super::{BlockDevice, BlockError, HardwareDisk};

pub struct Partition {
    disk: Box<dyn BlockDevice>,
    first: u64,
    sectors: u64,
}

impl Partition {
    pub fn new(disk: Box<dyn BlockDevice>, first: u64, sectors: u64) -> Result<Self, &'static str> {
        if sectors == 0 || first.checked_add(sectors).map_or(true, |end| end > disk.num_blocks()) {
            return Err("the partition does not fit its disk");
        }
        Ok(Self { disk, first, sectors })
    }

    pub fn first(&self) -> u64 {
        self.first
    }

    pub fn last(&self) -> u64 {
        self.first + self.sectors - 1
    }

    fn span(&self, start: u64, count: usize) -> Result<u64, BlockError> {
        let end = start.checked_add(count as u64).ok_or(BlockError::OutOfBounds)?;
        if end > self.sectors {
            return Err(BlockError::OutOfBounds);
        }
        Ok(self.first + start)
    }
}

impl BlockDevice for Partition {
    fn is_read_only(&self) -> bool {
        self.disk.is_read_only()
    }

    fn hardware_disk(&self) -> Option<HardwareDisk> {
        self.disk.hardware_disk()
    }

    /// Distinct from the whole disk's and from another partition's of the same disk.
    fn identity(&self) -> usize {
        self.disk.identity() ^ (self.first as usize).rotate_left(23) ^ 0x5041_5254
    }

    fn read_block(&self, block: u64, buf: &mut [u8]) -> Result<(), BlockError> {
        self.disk.read_block(self.span(block, 1)?, buf)
    }

    fn read_blocks(&self, start: u64, count: usize, buf: &mut [u8]) -> Result<(), BlockError> {
        self.disk.read_blocks(self.span(start, count)?, count, buf)
    }

    fn write_block(&self, block: u64, buf: &[u8]) -> Result<(), BlockError> {
        self.disk.write_block(self.span(block, 1)?, buf)
    }

    fn write_blocks(&self, start: u64, count: usize, buf: &[u8]) -> Result<(), BlockError> {
        self.disk.write_blocks(self.span(start, count)?, count, buf)
    }

    fn block_size(&self) -> usize {
        self.disk.block_size()
    }

    fn num_blocks(&self) -> u64 {
        self.sectors
    }

    fn flush(&self) -> Result<(), BlockError> {
        self.disk.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::sync::Arc;
    use alloc::vec;
    use alloc::vec::Vec;
    use spin::Mutex;

    struct Disk(Arc<Mutex<Vec<u8>>>);

    impl BlockDevice for Disk {
        fn read_block(&self, block: u64, buf: &mut [u8]) -> Result<(), BlockError> {
            self.read_blocks(block, 1, buf)
        }
        fn read_blocks(&self, start: u64, count: usize, buf: &mut [u8]) -> Result<(), BlockError> {
            let at = start as usize * 512;
            buf[..count * 512].copy_from_slice(&self.0.lock()[at..at + count * 512]);
            Ok(())
        }
        fn write_block(&self, block: u64, buf: &[u8]) -> Result<(), BlockError> {
            self.write_blocks(block, 1, buf)
        }
        fn write_blocks(&self, start: u64, count: usize, buf: &[u8]) -> Result<(), BlockError> {
            let at = start as usize * 512;
            self.0.lock()[at..at + count * 512].copy_from_slice(&buf[..count * 512]);
            Ok(())
        }
        fn block_size(&self) -> usize { 512 }
        fn num_blocks(&self) -> u64 { (self.0.lock().len() / 512) as u64 }
        fn flush(&self) -> Result<(), BlockError> { Ok(()) }
    }

    #[test]
    fn requests_are_relative_and_stay_inside() {
        let bytes = Arc::new(Mutex::new(vec![0u8; 100 * 512]));
        let part = Partition::new(Box::new(Disk(bytes.clone())), 10, 20).unwrap();
        part.write_blocks(0, 2, &[7u8; 1024]).unwrap();
        assert_eq!(bytes.lock()[10 * 512], 7);
        assert_eq!(bytes.lock()[12 * 512], 0);
        assert_eq!(part.write_blocks(19, 2, &[7u8; 1024]), Err(BlockError::OutOfBounds));
        assert_eq!(part.write_block(20, &[7u8; 512]), Err(BlockError::OutOfBounds));
        assert_eq!(part.read_blocks(u64::MAX, 1, &mut [0u8; 512]), Err(BlockError::OutOfBounds));
        assert_eq!(part.num_blocks(), 20);
        assert_eq!((part.first(), part.last()), (10, 29));
        assert!(Partition::new(Box::new(Disk(bytes)), 90, 20).is_err());
    }
}
