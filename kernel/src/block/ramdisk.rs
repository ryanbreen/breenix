//! A writable block device over the bootloader's reserved, writable RAM mapping.
//! Reads and writes mask interrupts around the device mutex and byte copy (issue #1702).

use alloc::{boxed::Box, sync::Arc};
use spin::{Mutex, Once};
use super::{BlockDevice, BlockError};

const SECTOR_SIZE: usize = 512;
static DISK: Once<RamDisk<'static>> = Once::new();

#[derive(Clone)]
struct RamDisk<'a> {
    bytes: Arc<Mutex<&'a mut [u8]>>,
    len: usize,
}

impl<'a> RamDisk<'a> {
    fn new(bytes: &'a mut [u8]) -> Result<Self, &'static str> {
        if bytes.is_empty() || bytes.len() % SECTOR_SIZE != 0 {
            return Err("RAM disk must contain whole nonempty sectors");
        }
        Ok(Self { len: bytes.len(), bytes: Arc::new(Mutex::new(bytes)) })
    }

    fn range(&self, start: u64, count: usize, capacity: usize) -> Result<core::ops::Range<usize>, BlockError> {
        let start = usize::try_from(start).map_err(|_| BlockError::OutOfBounds)?;
        let offset = start.checked_mul(SECTOR_SIZE).ok_or(BlockError::OutOfBounds)?;
        let bytes = count.checked_mul(SECTOR_SIZE).ok_or(BlockError::OutOfBounds)?;
        let end = offset.checked_add(bytes).ok_or(BlockError::OutOfBounds)?;
        if end > self.len { return Err(BlockError::OutOfBounds); }
        if bytes > capacity { return Err(BlockError::IoError); }
        Ok(offset..end)
    }
}

impl BlockDevice for RamDisk<'_> {
    fn identity(&self) -> usize { Arc::as_ptr(&self.bytes) as usize }
    fn read_block(&self, block: u64, buf: &mut [u8]) -> Result<(), BlockError> {
        self.read_blocks(block, 1, buf)
    }
    fn read_blocks(&self, start: u64, count: usize, buf: &mut [u8]) -> Result<(), BlockError> {
        let range = self.range(start, count, buf.len())?;
        crate::arch_without_interrupts(|| {
            buf[..range.len()].copy_from_slice(&self.bytes.lock()[range]);
        });
        Ok(())
    }
    fn write_block(&self, block: u64, buf: &[u8]) -> Result<(), BlockError> {
        self.write_blocks(block, 1, buf)
    }
    fn write_blocks(&self, start: u64, count: usize, buf: &[u8]) -> Result<(), BlockError> {
        let range = self.range(start, count, buf.len())?;
        crate::arch_without_interrupts(|| {
            self.bytes.lock()[range.clone()].copy_from_slice(&buf[..range.len()]);
        });
        Ok(())
    }
    fn block_size(&self) -> usize { SECTOR_SIZE }
    fn num_blocks(&self) -> u64 { (self.len / SECTOR_SIZE) as u64 }
    fn flush(&self) -> Result<(), BlockError> { Ok(()) }
}

/// Install the bootloader RAM disk after heap initialization, before mounting root.
///
/// # Safety
/// `addr..addr+len` must be a reserved, writable mapping valid for the kernel's
/// lifetime, exclusively handed to this device. claim-lint:ok: #1702; this is
/// the unsafe caller contract. BootInfo supplies that mapping;
/// its pages are not usable regions in the frame allocator's memory map.
pub unsafe fn init(addr: Option<u64>, len: u64) -> Result<(), &'static str> {
    let Some(addr) = addr else {
        return if len == 0 { Ok(()) } else { Err("RAM disk length without an address") };
    };
    let addr = usize::try_from(addr).map_err(|_| "RAM disk address overflow")?;
    let len = usize::try_from(len).map_err(|_| "RAM disk length overflow")?;
    if addr == 0 || len == 0 || len > isize::MAX as usize || addr.checked_add(len).is_none() {
        return Err("Invalid RAM disk mapping");
    }
    if DISK.get().is_some() { return Err("RAM disk already installed"); }
    // A disk-root console boot carries only its per-boot intent here. It
    // must not become a root block device or change the installed filesystem.
    let bytes = core::slice::from_raw_parts_mut(addr as *mut u8, len);
    if crate::boot::target::capture(bytes) {
        return Ok(());
    }
    let disk = RamDisk::new(bytes)?;
    DISK.call_once(|| disk);
    log::info!("[block] ramdisk {} bytes", len);
    Ok(())
}

pub fn device() -> Option<Box<dyn BlockDevice>> {
    DISK.get().map(|disk| Box::new(disk.clone()) as Box<dyn BlockDevice>)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_writes_and_multisector_reads() {
        let mut storage = [0; 2048];
        let disk = RamDisk::new(&mut storage).unwrap();
        let other = disk.clone();
        assert_eq!(disk.identity(), other.identity());
        let input = [0x5a; 1024];
        disk.write_blocks(1, 2, &input).unwrap();
        let mut output = [0xff; 1536];
        other.read_blocks(0, 3, &mut output).unwrap();
        assert_eq!(&output[..512], &[0; 512]);
        assert_eq!(&output[512..], &input);
        disk.flush().unwrap();
    }

    #[test]
    fn refused_writes_preserve_data() {
        let mut storage = [0; 2048];
        let disk = RamDisk::new(&mut storage).unwrap();
        for (start, count, capacity) in [(3, 2, 1024), (u64::MAX, 1, 512), (0, usize::MAX, 512), (0, 2, 512)] {
            assert!(disk.write_blocks(start, count, &alloc::vec![0x5a; capacity]).is_err());
        }
        let mut bytes = [0xff; 2048];
        disk.read_blocks(0, 4, &mut bytes).unwrap();
        assert_eq!(bytes, [0; 2048]);
        assert_eq!(disk.read_block(4, &mut bytes), Err(BlockError::OutOfBounds));
        assert_eq!(disk.read_block(0, &mut bytes[..511]), Err(BlockError::IoError));
    }

    #[test]
    fn rejects_empty_and_partial_sector_images() {
        let mut storage = [0; 513];
        for len in [0, 1, 511, 513] {
            assert!(RamDisk::new(&mut storage[..len]).is_err());
        }
    }
}
