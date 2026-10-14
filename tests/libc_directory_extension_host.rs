//! Exercise the real extension transaction against sector write failures on ARM hosts.
extern crate alloc;
extern crate self as spin;
pub struct Mutex<T>(std::sync::Mutex<T>);
impl<T> Mutex<T> {
    pub fn new(value: T) -> Self {
        Self(std::sync::Mutex::new(value))
    }
    pub fn lock(&self) -> std::sync::MutexGuard<'_, T> {
        self.0.lock().unwrap()
    }
}
mod block {
    #[derive(Debug)]
    pub enum BlockError {
        IoError,
        OutOfMemory,
    }
    pub trait BlockDevice: Send + Sync {
        fn identity(&self) -> usize {
            self as *const Self as *const () as usize
        }
        fn block_size(&self) -> usize;
        fn num_blocks(&self) -> u64;
        fn flush(&self) -> Result<(), BlockError>;
        fn read_block(&self, sector: u64, bytes: &mut [u8]) -> Result<(), BlockError>;
        fn write_block(&self, sector: u64, bytes: &[u8]) -> Result<(), BlockError>;
    }
}
struct Ext2Superblock;
impl Ext2Superblock {
    fn block_size(&self) -> usize {
        512
    }
}
struct Ext2BlockGroupDesc {
    bg_block_bitmap: u32,
}
#[path = "../kernel/src/fs/ext2/directory_extension.rs"]
mod directory_extension;
use block::{BlockDevice, BlockError};
struct Disk {
    bytes: Mutex<[[u8; 512]; 4]>,
    writes: Mutex<usize>,
    fail: usize,
    restore_fail: bool,
}
impl BlockDevice for Disk {
    fn block_size(&self) -> usize {
        512
    }
    fn num_blocks(&self) -> u64 {
        4
    }
    fn flush(&self) -> Result<(), BlockError> {
        Ok(())
    }
    fn read_block(&self, n: u64, b: &mut [u8]) -> Result<(), BlockError> {
        b[..512].copy_from_slice(&self.bytes.lock()[n as usize]);
        Ok(())
    }
    fn write_block(&self, n: u64, b: &[u8]) -> Result<(), BlockError> {
        let mut count = self.writes.lock();
        *count += 1;
        if self.restore_fail && *count > self.fail && n == 1 {
            return Err(BlockError::IoError);
        }
        // A reported error can follow bytes reaching the disk.
        self.bytes.lock()[n as usize].copy_from_slice(&b[..512]);
        if *count == self.fail {
            Err(BlockError::IoError)
        } else {
            Ok(())
        }
    }
}
#[test]
fn partial_inode_or_pointer_failure_restores_before_recycling() {
    for fail in 1..=3 {
        let disk = Disk {
            bytes: Mutex::new([[0; 512]; 4]),
            writes: Mutex::new(0),
            fail,
            restore_fail: false,
        };
        let transaction = directory_extension::Extension::new(
            &disk,
            &Ext2Superblock,
            &[Ext2BlockGroupDesc { bg_block_bitmap: 0 }],
        )
        .unwrap();
        assert_eq!(transaction.identity(), disk.identity());
        assert_eq!(transaction.num_blocks(), 4);
        transaction.flush().unwrap();
        transaction.write_block(0, &[1; 512]).unwrap();
        transaction.write_block(1, &[2; 512]).unwrap();
        transaction.write_block(2, &[3; 512]).unwrap();
        assert_eq!(*disk.bytes.lock(), [[0; 512]; 4]);
        assert_eq!(transaction.publish(), Err(true));
        assert_eq!(*disk.bytes.lock(), [[0; 512]; 4]);
    }
}
#[test]
fn failed_reference_restore_retains_allocation() {
    let disk = Disk {
        bytes: Mutex::new([[0; 512]; 4]),
        writes: Mutex::new(0),
        fail: 2,
        restore_fail: true,
    };
    let transaction = directory_extension::Extension::new(
        &disk,
        &Ext2Superblock,
        &[Ext2BlockGroupDesc { bg_block_bitmap: 0 }],
    )
    .unwrap();
    transaction.write_block(0, &[1; 512]).unwrap();
    transaction.write_block(1, &[2; 512]).unwrap();
    assert_eq!(transaction.publish(), Err(false));
    assert_eq!(disk.bytes.lock()[0], [1; 512]);
}
