//! Hardware write barrier applied to discovered disks on physical x86 boots.

use alloc::boxed::Box;
use super::{BlockDevice, BlockError};

struct ReadOnly(Box<dyn BlockDevice>);

pub(super) fn device(device: Box<dyn BlockDevice>) -> Box<dyn BlockDevice> {
    Box::new(ReadOnly(device))
}

impl BlockDevice for ReadOnly {
    fn is_read_only(&self) -> bool { true }
    fn hardware_disk(&self) -> Option<super::HardwareDisk> { self.0.hardware_disk() }

    fn identity(&self) -> usize { self.0.identity() }
    fn read_block(&self, block: u64, buf: &mut [u8]) -> Result<(), BlockError> {
        self.0.read_block(block, buf)
    }
    fn read_blocks(&self, start: u64, count: usize, buf: &mut [u8]) -> Result<(), BlockError> {
        self.0.read_blocks(start, count, buf)
    }
    fn write_block(&self, _block: u64, _buf: &[u8]) -> Result<(), BlockError> {
        Err(BlockError::IoError)
    }
    fn write_blocks(&self, _start: u64, _count: usize, _buf: &[u8]) -> Result<(), BlockError> {
        Err(BlockError::IoError)
    }
    fn block_size(&self) -> usize { self.0.block_size() }
    fn num_blocks(&self) -> u64 { self.0.num_blocks() }
    fn flush(&self) -> Result<(), BlockError> { Ok(()) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::sync::Arc;
    use core::sync::atomic::{AtomicUsize, Ordering};

    struct Disk(Arc<AtomicUsize>);
    impl BlockDevice for Disk {
        fn read_block(&self, _block: u64, buf: &mut [u8]) -> Result<(), BlockError> {
            buf.fill(0x5a);
            Ok(())
        }
        fn write_block(&self, _block: u64, _buf: &[u8]) -> Result<(), BlockError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        fn block_size(&self) -> usize { 512 }
        fn num_blocks(&self) -> u64 { 2 }
        fn flush(&self) -> Result<(), BlockError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    #[test]
    fn discovered_hardware_refuses_writes_before_reaching_backend() {
        let writes = Arc::new(AtomicUsize::new(0));
        let backend = Box::new(Disk(writes.clone()));
        let identity = backend.identity();
        let disk = device(backend);
        assert_eq!(disk.identity(), identity);
        assert!(disk.is_read_only());
        let mut bytes = [0; 1024];
        disk.read_blocks(0, 2, &mut bytes).unwrap();
        assert_eq!(bytes, [0x5a; 1024]);
        assert_eq!(disk.write_block(0, &bytes[..512]), Err(BlockError::IoError));
        assert_eq!(disk.write_blocks(0, 2, &bytes), Err(BlockError::IoError));
        assert_eq!(disk.write_blocks(0, 0, &[]), Err(BlockError::IoError));
        disk.flush().unwrap();
        assert_eq!(writes.load(Ordering::SeqCst), 0);
        assert_eq!(disk.block_size(), 512);
        assert_eq!(disk.num_blocks(), 2);
    }
}
