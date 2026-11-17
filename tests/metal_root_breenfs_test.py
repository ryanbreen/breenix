#!/usr/bin/env python3
"""Exercise conversion of a staged metal root through the real host tools."""
import hashlib
import os
from pathlib import Path
import subprocess
import struct
import shutil
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'scripts/metal'))
import metal_root


def crc32c(data):
    crc = 0xffffffff
    for byte in data:
        crc ^= byte
        for _ in range(8):
            crc = (crc >> 1) ^ (0x82f63b78 if crc & 1 else 0)
    return crc ^ 0xffffffff


def valid_ring_a_slots(image):
    # breenfs-format.md sections 2 and 2.5; coordinator ruling E24 line 458.
    slots = []
    with image.open('rb') as disk:
        for slot in range(4):
            disk.seek((16 + slot) * 4096)
            data = disk.read(4096)
            if len(data) != 4096 or data[:8] != b'BREENFSS':
                continue
            generation = struct.unpack_from('<Q', data, 32)[0]
            if (crc32c(data[12:]) == struct.unpack_from('<I', data, 8)[0]
                    and generation and generation % 4 == slot and data[40:42] == bytes([0, slot])):
                slots.append((generation, slot))
    return slots


class BreenfsRootTest(unittest.TestCase):
    def test_staged_root_and_install_record(self):
        mke2fs, = metal_root.require_e2fsprogs('mke2fs')
        tools = Path(os.environ.get('BREENFS_TOOLS_DIR', ROOT / 'tools/breenfs/target/release'))
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            tree = root / 'tree'
            for directory in ('etc/breenix', 'sbin', 'bin', 'mnt'):
                (tree / directory).mkdir(parents=True)
            (tree / 'sbin/suite-smoke').write_bytes(b'\x7fELFfixture')
            (tree / 'sbin/init').write_bytes(b'\x7fELFfixture')
            image = root / 'root.img'
            subprocess.run([mke2fs, '-q', '-t', 'ext2', '-d', str(tree), '-F', str(image), '65536'],
                           check=True, capture_output=True)
            facts = metal_root.stage_root(image, 'suite', 'smoke', {}, root_fs='breenfs', tools_dir=tools)
            output = subprocess.check_output([str(tools / 'breenfs-extract'), str(image), 'cat', '/etc/breenix/root-fs'])
            self.assertEqual(output, b'root-fs breenfs\n')
            subprocess.run([str(tools / 'breenfs-check'), '--full', str(image)], check=True)
            with image.open('rb') as disk:
                self.assertEqual(disk.read(16 * 4096), bytes(16 * 4096))
            slots = valid_ring_a_slots(image)
            self.assertTrue(slots)
            newest = max(slots)
            self.assertEqual(newest[0] % 4, newest[1])
            slot2 = root / 'slot2.img'
            shutil.copyfile(image, slot2)
            with slot2.open('r+b') as disk:
                disk.seek((16 + newest[1]) * 4096)
                block = bytearray(disk.read(4096))
                generation = newest[0] + (2 - newest[0]) % 4
                struct.pack_into('<Q', block, 32, generation)
                block[41] = 2
                struct.pack_into('<I', block, 8, crc32c(block[12:]))
                disk.seek(16 * 4096)
                disk.write(bytes(4 * 4096))
                disk.seek(18 * 4096)
                disk.write(block)
            self.assertEqual(valid_ring_a_slots(slot2), [(generation, 2)])
            metal_root.write_install_record(slot2, 'slot2')
            invalid = root / 'invalid.img'
            shutil.copyfile(image, invalid)
            with invalid.open('r+b') as disk:
                disk.seek(16 * 4096)
                disk.write(bytes(4 * 4096))
            with self.assertRaises(ValueError):
                metal_root.write_install_record(invalid, 'invalid')
            self.assertGreater(facts['extentCount'], 0)
            self.assertEqual(facts['breenfsSha256'], hashlib.sha256(image.read_bytes()).hexdigest())
            record = metal_root.write_install_record(image, 'fixture')
            with image.open('rb') as disk:
                self.assertEqual(disk.read(512), metal_root.install_record('fixture'))
            self.assertTrue(metal_root.root_record('fixture', record['bytes'], record['sha256AfterRecord'])
                            .startswith(b'BREENIX-STORAGE 1\nroot fixture '))


if __name__ == '__main__':
    unittest.main()
