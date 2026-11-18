#!/usr/bin/env python3
"""Compile the guest's root contracts on the host, without a boot."""
import re
import subprocess
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


class RootExpectationsTest(unittest.TestCase):
    def test_root_contracts(self):
        # E24 ledger 520(q): conversion preserves the source fixture's names.
        builder = (ROOT / 'scripts/create_ext2_disk.sh').read_text()
        loop = re.search(
            r'i=0\s+while \[ \$\(\(i \* (\d+)\)\) -le '
            r'\$\(\((\d+) \* fixture_block_size\)\) \]; do'
            r'(.*?)i=\$\(\(i \+ 1\)\)', builder, re.S)
        self.assertIsNotNone(loop, 'cookie fixture builder loop not found')
        record_bytes, blocks = map(int, loop.group(1, 2))
        fixture_entries = len(range(0, blocks * 1024 + 1, record_bytes))
        case = (ROOT / 'userspace/programs/src/suite_directories.rs').read_text()
        case = case.split('fn readdir_cookie_blocks() -> CaseResult {', 1)[1]
        count = re.search(r'let n = ([^;]+);', case)
        self.assertIsNotNone(count, 'cookie case fixture count not found')
        with tempfile.TemporaryDirectory() as tmp:
            source = Path(tmp) / 'expectations.rs'
            source.write_text(f'''
#[path = "{ROOT}/userspace/programs/src/filesystem_root.rs"]
pub mod filesystem_root;
#[path = "{ROOT}/userspace/programs/src/root_expectations.rs"]
pub mod root_expectations;
use filesystem_root::RootFs;
use root_expectations::{{for_root, CookieEncoding}};

fn main() {{
    // Source construction, independent of the mounted root's block geometry.
    assert_eq!({fixture_entries}, 15);
    for block in [1024_u64, 4096] {{
        let n = {count.group(1)};
        assert_eq!(n, {fixture_entries}, "fixture count on {{block}}-byte root");
    }}
    let ext2 = for_root(RootFs::Ext2);
    assert!(ext2.reuses_inode_numbers);
    assert_eq!(ext2.directory_size(12288, 60), 12288);
    assert_eq!(ext2.cache_block_bytes, 512);
    assert_eq!(ext2.statfs_magic, 0xef53);
    // Current sparse pointer-depth, dense shrink and regrowth expectations.
    assert_eq!(ext2.blocks(4, 6, 8), 80);
    assert_eq!(ext2.blocks(3, 3, 8), 48);
    assert_eq!(ext2.blocks(2, 1, 8), 24);
    assert_eq!(ext2.blocks(129, 1, 8), 1040);
    assert_eq!(ext2.blocks(1, 0, 8), 8);
    assert_eq!(ext2.blocks(0, 0, 8), 0);

    assert_eq!(ext2.cookie_encoding, CookieEncoding::ByteOffsets);
    assert!(ext2.cookies_match(&[12, 24, 4200, 4408], 4096, 2));
    assert!(!ext2.cookies_match(&[12, 24, 200, 408], 4096, 2));
    assert_eq!(ext2.available_blocks(8000, 16384, 17), Some(8000));
    assert!(ext2.inode_capacity(100, 90));
    assert!(!ext2.inode_capacity(0, 0));

    let breenfs = for_root(RootFs::Breenfs);
    assert!(!breenfs.reuses_inode_numbers);
    // Design 2.3: size counts names, excluding synthetic dot entries.
    assert_eq!(breenfs.directory_size(12288, 60), 60);
    assert_eq!(breenfs.directory_size(4096, 0), 0);
    // Design 1.2 and 2: verified buffers contain native 4096-byte blocks.
    assert_eq!(breenfs.cache_block_bytes, 4096);
    assert_eq!(1048576 / breenfs.cache_block_bytes, 256);
    assert_eq!(1048576 / ext2.cache_block_bytes, 2048);
    // Design 2.3 and F7: st_blocks counts mapped data, no pointer blocks.
    assert_eq!(breenfs.blocks(4, 6, 8), 32);
    assert_eq!(breenfs.blocks(3, 3, 8), 24);
    assert_eq!(breenfs.blocks(2, 1, 8), 16);
    assert_eq!(breenfs.blocks(129, 1, 8), 1032);
    assert_eq!(breenfs.blocks(1, 0, 8), 8);
    assert_eq!(breenfs.blocks(0, 0, 8), 0);
    assert_eq!(breenfs.cookie_encoding, CookieEncoding::DirectoryIndexes);
    assert!(breenfs.cookies_match(&[1, 2, 3, 4, 5], 4096, 3));
    assert!(!breenfs.cookies_match(&[1, 2, 3, 3, 5], 4096, 3));
    assert!(!breenfs.cookies_match(&[1, 2, 3, 4, 6], 4096, 3));
    assert!(!breenfs.cookies_match(&[0, 2, 3], 4096, 1));
    // Design 2.8: total/200, clamped to 16 MiB..1 GiB, plus outstanding grants.
    assert_eq!(breenfs.available_blocks(8000, 16384, 17), Some(3887));
    assert_eq!(breenfs.available_blocks(100000, 10000000, 7), Some(49993));
    assert_eq!(breenfs.available_blocks(1000000, 100000000, 0), Some(737856));
    assert_eq!(breenfs.available_blocks(4095, 16384, 0), None);
    assert!(breenfs.inode_capacity(0, 0));
    assert!(!breenfs.inode_capacity(100, 90));
    // Design 2.1: filesystem identity, also required for a contained FIFO.
    assert_eq!(breenfs.statfs_magic, u64::from_le_bytes(*b"BREENFSS"));
}}
''')
            binary = Path(tmp) / 'expectations'
            subprocess.run(['taskpolicy', '-b', 'rustc', '--edition=2021', '-Dwarnings',
                            str(source), '-o', str(binary)], check=True)
            subprocess.run([str(binary)], check=True)


if __name__ == '__main__':
    unittest.main()
