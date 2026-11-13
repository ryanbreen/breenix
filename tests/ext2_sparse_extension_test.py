"""Count production sparse-extension I/O without booting or timing the kernel."""
from pathlib import Path
import re
import unittest

from file_map_orphan_cache_test import run
from gt710_gr_live_unwind_test import item

ROOT = Path(__file__).resolve().parents[1]


class SparseExtension(unittest.TestCase):
    def test_extension_transfers_and_allocates_no_data_blocks(self):
        source = (ROOT / 'kernel/src/fs/ext2/file.rs').read_text()
        functions = '\n'.join(item(source, marker) for marker in [
            'pub fn resize_file<', 'pub fn get_block_num<',
            'pub fn read_file_range<', 'fn read_indirect_block<',
            'pub fn read_ext2_block<', 'pub fn read_ext2_blocks<',
            'pub fn write_ext2_block<',
        ])
        constants = '\n'.join(re.findall(
            r'^const (?:DIRECT_BLOCKS|SINGLE_INDIRECT|DOUBLE_INDIRECT|TRIPLE_INDIRECT):[^;]+;',
            source, re.MULTILINE))
        result = run(r'''
use std::sync::Mutex;
#[derive(Debug)] pub enum BlockError { IoError, OutOfMemory }
pub trait BlockDevice {
    fn block_size(&self) -> usize;
    fn read_blocks(&self, start: u64, count: usize, bytes: &mut [u8]) -> Result<(), BlockError>;
    fn write_blocks(&self, start: u64, count: usize, bytes: &[u8]) -> Result<(), BlockError>;
}
// In-memory inode/superblock fields and device boundary; addressing, extension,
// and ranged reads above that boundary are the production functions below.
pub struct Ext2Inode { i_size: u32, i_dir_acl: u32, i_blocks: u32, i_block: [u32; 15] }
impl Ext2Inode {
    fn size(&self) -> u64 { self.i_size as u64 | ((self.i_dir_acl as u64) << 32) }
    fn update_timestamps(&mut self, _access: bool, _modify: bool, _change: bool) {}
}
pub struct Ext2Superblock { s_blocks_count: u32 }
impl Ext2Superblock { fn block_size(&self) -> usize { 4096 } }
struct Disk { io: Mutex<(usize, usize)> }
impl BlockDevice for Disk {
    fn block_size(&self) -> usize { 512 }
    fn read_blocks(&self, _start: u64, count: usize, bytes: &mut [u8]) -> Result<(), BlockError> {
        self.io.lock().unwrap().0 += count; bytes.fill(0xa5); Ok(())
    }
    fn write_blocks(&self, _start: u64, count: usize, _bytes: &[u8]) -> Result<(), BlockError> {
        self.io.lock().unwrap().1 += count; Ok(())
    }
}
CONSTANTS
FUNCTIONS
fn main() {
    let disk = Disk { io: Mutex::new((0, 0)) };
    let sb = Ext2Superblock { s_blocks_count: 512 };
    let mut inode = Ext2Inode { i_size: 0, i_dir_acl: 0, i_blocks: 0, i_block: [0; 15] };
    // Include a desktop arena-sized extent and a much larger sparse file.
    for length in [4096, 33_832_960, 1 << 30] {
        resize_file(&disk, &mut inode, &sb, length).unwrap();
        assert_eq!(inode.size(), length);
        assert_eq!(inode.i_blocks, 0, "extending EOF allocated data blocks");
        assert_eq!(inode.i_block, [0; 15], "extending EOF installed block pointers");
        assert_eq!(*disk.io.lock().unwrap(), (0, 0), "extending EOF transferred data");
        for offset in [0, (length / 2) & !4095, length - 4096] {
            let bytes = read_file_range(&disk, &inode, &sb, offset, 4096).unwrap();
            assert_eq!(bytes, [0; 4096], "sparse extension exposed old disk bytes");
        }
        assert_eq!(*disk.io.lock().unwrap(), (0, 0), "reading holes accessed the disk");
    }
}
'''.replace('CONSTANTS', constants).replace('FUNCTIONS', functions))
        self.assertEqual(result.returncode, 0, result.stderr)


if __name__ == '__main__':
    unittest.main()
