#!/usr/bin/env python3
"""Compile the guest's root contracts on the host, without a boot."""
import os
import re
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'scripts/metal'))
import metal_root

COOKIE_RECEIPT = '/usr/libexec/breenix/test/dir-blocks-count'


class RootExpectationsTest(unittest.TestCase):
    def test_cookie_fixture_receipt(self):
        builder = (ROOT / 'scripts/create_ext2_disk.sh').read_text()
        docker_start = 'mkdir -p /mnt/ext2/test/dir-blocks\n'
        native_start = 'mkdir -p "$MOUNT_DIR/test/dir-blocks"\n'
        fragments = (
            (docker_start + builder.split(docker_start, 1)[1]
             .split('# Create additional test content', 1)[0]).replace('/mnt/ext2', '${MOUNT_DIR}'),
            native_start + builder.split(native_start, 1)[1]
            .split('mkdir -p "$MOUNT_DIR/deep/path/to/file"', 1)[0],
        )
        mke2fs, debugfs = metal_root.require_e2fsprogs('mke2fs', 'debugfs')
        tools = Path(os.environ.get('BREENFS_TOOLS_DIR', ROOT / 'tools/breenfs/target/release'))
        for block_size, expected in ((1024, 15), (4096, 60)):
            for fragment in fragments:
                with self.subTest(block_size=block_size, builder=fragment.splitlines()[0]):
                    with tempfile.TemporaryDirectory() as tmp:
                        work = Path(tmp)
                        tree = work / 'tree'
                        (tree / 'etc').mkdir(parents=True)
                        env = dict(os.environ, MOUNT_DIR=str(tree), fixture_block_size=str(block_size),
                                   BREENIX_EXT2_OWNERS=str(work / 'owners'))
                        subprocess.run(['sh', '-eu', '-c', fragment], env=env, check=True)
                        subprocess.run(
                            ['sh', str(ROOT / 'scripts/install-layout.sh'), str(tree), str(work),
                             str(ROOT / 'libs/libbreenix/src/layout.tsv')], env=env, check=True)
                        receipt = tree / COOKIE_RECEIPT.lstrip('/')
                        names = sorted(receipt.with_name('dir-blocks').iterdir())
                        self.assertEqual([name.name for name in names],
                                         [f'{i:0200d}' for i in range(expected)])
                        self.assertEqual(receipt.read_bytes(), f'{expected}\n'.encode())
                        image = work / 'root.img'
                        subprocess.run([mke2fs, '-q', '-t', 'ext2', '-b', str(block_size), '-d', str(tree),
                                        '-F', str(image), str((32 << 20) // block_size)],
                                       check=True, capture_output=True)
                        metal_root.convert_breenfs(image, tools, debugfs)
                        output = subprocess.check_output(
                            [str(tools / 'breenfs-extract'), str(image), 'cat', COOKIE_RECEIPT])
                        self.assertEqual(output, receipt.read_bytes())

    def test_root_contracts(self):
        # Read the same receipt through the production case's count expression.
        case = (ROOT / 'userspace/programs/src/suite_directories.rs').read_text()
        case = case.split('fn readdir_cookie_blocks() -> CaseResult {', 1)[1]
        count = re.search(r'let n = ([^;]+);', case)
        self.assertIsNotNone(count, 'cookie case fixture count not found')
        expression = count.group(1).replace(f'"{COOKIE_RECEIPT}"', 'receipt')
        with tempfile.TemporaryDirectory() as tmp:
            source = Path(tmp) / 'expectations.rs'
            source.write_text(f'''
#[path = "{ROOT}/userspace/programs/src/filesystem_root.rs"]
pub mod filesystem_root;
#[path = "{ROOT}/userspace/programs/src/root_expectations.rs"]
pub mod root_expectations;
use filesystem_root::RootFs;
use root_expectations::{{for_root, CookieEncoding}};

fn fixture_entries(receipt: &str) -> Result<usize, String> {{
    let _ = receipt;
    Ok({expression})
}}

fn main() {{
    let receipt = std::env::args().nth(1).unwrap();
    for (block, entries) in [(1024, 15), (4096, 60)] {{
        std::fs::write(&receipt, format!("{{entries}}\\n")).unwrap();
        assert_eq!(fixture_entries(&receipt).unwrap(), entries,
            "fixture count from the {{block}}-byte seed");
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
            subprocess.run([str(binary), str(Path(tmp) / 'dir-blocks-count')], check=True)


if __name__ == '__main__':
    unittest.main()
