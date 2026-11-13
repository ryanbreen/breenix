"""Count namespace-create transfers in production ext2, without clocks or boots."""
from pathlib import Path
import re
import unittest

from file_map_orphan_cache_test import run
from gt710_gr_live_unwind_test import item

ROOT = Path(__file__).resolve().parents[1]
EXT2 = ROOT / 'kernel/src/fs/ext2'


def production():
    file = (EXT2 / 'file.rs').read_text()
    functions = '\n'.join(item(file, marker) for marker in [
        'pub fn get_block_num<', 'pub fn read_file<', 'pub fn read_file_range<',
        'fn read_indirect_block<', 'pub fn read_ext2_block<',
        'pub fn read_ext2_blocks<', 'pub fn write_ext2_block<',
    ])
    constants = '\n'.join(re.findall(
        r'^const (?:DIRECT_BLOCKS|SINGLE_INDIRECT|DOUBLE_INDIRECT|TRIPLE_INDIRECT):[^;]+;',
        file, re.MULTILINE))
    group = (EXT2 / 'block_group.rs').read_text()
    inode = (EXT2 / 'inode.rs').read_text()
    io = (EXT2 / 'directory_io.rs').read_text()
    return r'''
extern crate alloc;
pub mod block { pub struct HardwareDisk; @BLOCK@ }
pub mod time {
    pub fn current_unix_time() -> u64 {
        static NOW: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        NOW.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    }
}
pub mod fs {
    #[path = "@ATTRS@"] pub mod attrs;
    pub mod ext2 {
        use alloc::vec::Vec;
        use crate::block::BlockDevice;
        #[path = "@SUPERBLOCK@"] pub mod superblock;
        #[path = "@DIR@"] pub mod dir;
        pub use superblock::Ext2Superblock;
        pub use block_group::Ext2BlockGroupDesc;
        pub use inode::*;
        pub use dir::*;
        pub mod inode { @INODE@ }
        pub mod file {
            use alloc::vec::Vec;
            use crate::block::{BlockDevice, BlockError};
            use super::{Ext2Inode, Ext2Superblock};
            @CONSTANTS@
            @FUNCTIONS@
        }
        pub mod block_group {
            use alloc::vec::Vec;
            use core::mem;
            use crate::block::{BlockDevice, BlockError};
            use super::{Ext2Superblock, file::read_ext2_block};
            @GROUP@
        }
        // No pending reclamation/quarantine in this namespace-create fixture.
        pub mod reclaim {
            pub fn quarantined<B: BlockDevice + ?Sized>(_d: &B, _inode: bool, _n: u32) -> bool { false }
            use crate::block::BlockDevice;
        }
        pub struct Ext2Fs {
            pub device: Box<dyn BlockDevice>,
            pub superblock: Ext2Superblock,
            pub block_groups: Vec<Ext2BlockGroupDesc>,
        }
        impl Ext2Fs {
            pub fn read_inode(&self, ino: u32) -> Result<Ext2Inode, &'static str> {
                Ext2Inode::read_from(self.device.as_ref(), ino, &self.superblock, &self.block_groups).map_err(|_| "read")
            }
            pub fn read_directory(&self, inode: &Ext2Inode) -> Result<Vec<u8>, &'static str> {
                file::read_file(self.device.as_ref(), inode, &self.superblock).map_err(|_| "directory")
            }
            pub fn write_inode(&mut self, ino: u32, inode: &Ext2Inode) -> Result<(), &'static str> {
                self.write_inode_disk(ino, inode)
            }
            fn write_inode_disk(&self, ino: u32, inode: &Ext2Inode) -> Result<(), &'static str> {
                inode.write_to(self.device.as_ref(), ino, &self.superblock, &self.block_groups).map_err(|_| "write")
            }
            // This check covers insertion with existing room, as on the A.4 disk.
            fn make_directory_room(&mut self, _ino: u32, _inode: &mut Ext2Inode, data: &mut Vec<u8>, len: usize) -> Result<(), &'static str> {
                assert!(dir::has_room(data, len)?); Ok(())
            }
            fn fail_writes(&mut self) { panic!("fixture undo failed"); }
        }
        pub mod directory_io {
            use super::{file, Ext2BlockGroupDesc, Ext2Fs, Ext2Inode, Ext2Superblock};
            use crate::block::BlockError;
            use alloc::vec::Vec;
            @IO@
        }
        #[path = "@CREATE@"] pub mod create_node;
    }
}
'''.replace('@BLOCK@', (ROOT / 'tools/ext2-host-tests/src/block.rs').read_text().split('#[path =')[0]) \
        .replace('@ATTRS@', str(ROOT / 'kernel/src/fs/attrs.rs')) \
        .replace('@SUPERBLOCK@', str(EXT2 / 'superblock.rs')) \
        .replace('@DIR@', str(EXT2 / 'dir.rs')) \
        .replace('@CREATE@', str(EXT2 / 'create_node.rs')) \
        .replace('@INODE@', inode[:inode.index('/// Take a freed directory')].replace('use crate::fs::ext2::file::{read_ext2_block, write_ext2_block};', '')
                 + item(inode[inode.index('/// Take a freed directory'):], 'impl Ext2Inode {')) \
        .replace('@CONSTANTS@', constants).replace('@FUNCTIONS@', functions) \
        .replace('@GROUP@', '#[repr(C, packed)]\n#[derive(Debug, Clone, Copy)]\n'
                 + item(group, 'pub struct Ext2BlockGroupDesc')
                 + item(group, 'impl Ext2BlockGroupDesc {')
                 + item(group, 'pub(super) struct Claim')
                 + item(group, 'pub(super) fn first_free_bit<')
                 + item(group, 'pub(super) fn claim<')) \
        .replace('@IO@', item(io, 'pub(super) enum Write')
                 + item(io, 'pub(super) fn copy<')
                 + 'impl Ext2Fs {'
                 + item(io, 'pub(super) fn commit_writes(')
                 + item(io, 'pub(super) fn plan_blocks<')
                 + item(io, 'fn apply(') + '}')


class CreateIo(unittest.TestCase):
    def test_create_writes_only_changed_counts_and_undo_restores_them(self):
        result = run(production() + (ROOT / 'tests/fixtures/ext2_create_io.rs').read_text())
        print(result.stdout, end='')
        self.assertEqual(result.returncode, 0, result.stderr)


if __name__ == '__main__':
    unittest.main()
