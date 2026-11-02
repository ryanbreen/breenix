"""Check the built ARM watchdog's MMIO instruction forms for HVF."""
from pathlib import Path
import os
import re
import shutil
import subprocess
import unittest

ROOT = Path(__file__).resolve().parents[1]


class WatchdogMmio(unittest.TestCase):
    def test_feed_stores_have_no_register_writeback(self):
        kernel = Path(os.environ.get('BREENIX_ARM_KERNEL', ROOT / 'target/aarch64-breenix-kernel/release/kernel-aarch64'))
        llvm = Path('/opt/homebrew/opt/llvm/bin')
        nm = shutil.which('llvm-nm') or str(llvm / 'llvm-nm')
        objdump = shutil.which('llvm-objdump') or str(llvm / 'llvm-objdump')
        symbols = subprocess.check_output([nm, '-n', '-C', str(kernel)], text=True).splitlines()
        feeds = [i for i, line in enumerate(symbols) if 'kernel::drivers::watchdog_round::feed::' in line]
        self.assertEqual(len(feeds), 1, 'expected one ARM watchdog feeding-policy instantiation')
        start = int(symbols[feeds[0]].split()[0], 16)
        end = next(int(line.split()[0], 16) for line in symbols[feeds[0] + 1:]
                   if int(line.split()[0], 16) > start)
        assembly = subprocess.check_output([objdump, '-d', '--start-address=' + hex(start),
                                            '--stop-address=' + hex(end), str(kernel)], text=True)
        # A writeback store to an emulated device can report ISV=0. HVF requires
        # ISV=1 to decode the transfer. Stack-frame writeback is normal RAM and
        # is excluded; the feeding path's device stores must use fixed addresses.
        stores = [line for line in assembly.splitlines()
                  if re.search(r'\bstr(?:b|h)?\s', line) and '[sp' not in line]
        self.assertTrue(stores, assembly)
        self.assertFalse(any('!' in line or re.search(r'\],\s*#', line) for line in stores), assembly)


if __name__ == '__main__':
    unittest.main()
