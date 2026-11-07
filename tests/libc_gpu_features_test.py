#!/usr/bin/env python3
"""The core-only libc build must not pull in the allocating GPU codec."""
import subprocess
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


class LibcGpuFeaturesTest(unittest.TestCase):
    def test_libc_does_not_enable_gpu_alloc(self):
        tree = subprocess.check_output([
            'cargo', 'tree', '--manifest-path', str(ROOT / 'libs/libbreenix-libc/Cargo.toml'),
            '-e', 'features', '-i', 'gpu-abi',
        ], text=True)
        self.assertNotIn('gpu-abi feature "alloc"', tree)


if __name__ == '__main__':
    unittest.main()
