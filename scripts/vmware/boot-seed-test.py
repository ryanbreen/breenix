#!/usr/bin/env python3
"""Check the real host seed writer without booting a VM."""
import importlib.util
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('boot_seed', Path(__file__).with_name('boot-seed.py'))
boot_seed = importlib.util.module_from_spec(spec)
spec.loader.exec_module(boot_seed)


class HostSeedTest(unittest.TestCase):
    def test_each_write_replaces_the_previous_seed_with_64_fresh_bytes(self):
        with tempfile.TemporaryDirectory() as work:
            seed = Path(work) / 'seed'
            boot_seed.write_seed(seed)
            first = seed.read_bytes()
            boot_seed.write_seed(seed)
            second = seed.read_bytes()
            self.assertEqual(len(first), 64)
            self.assertEqual(len(second), 64)
            self.assertNotEqual(first, second)


if __name__ == '__main__':
    unittest.main()
