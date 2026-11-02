"""Check the built ARM watchdog's MMIO instruction forms for HVF."""
from pathlib import Path
import os
import re
import shutil
import subprocess
import unittest

ROOT = Path(__file__).resolve().parents[1]


class WatchdogMmio(unittest.TestCase):
    def test_device_accesses_have_no_register_writeback(self):
        kernel = Path(os.environ.get('BREENIX_ARM_KERNEL', ROOT / 'target/aarch64-breenix-kernel/release/kernel-aarch64'))
        llvm = Path('/opt/homebrew/opt/llvm/bin')
        nm = shutil.which('llvm-nm') or str(llvm / 'llvm-nm')
        objdump = shutil.which('llvm-objdump') or str(llvm / 'llvm-objdump')
        symbols = subprocess.check_output([nm, '-n', '-C', str(kernel)], text=True).splitlines()
        paths = [i for i, line in enumerate(symbols)
                 if 'kernel::drivers::watchdog_round::feed::' in line
                 or 'kernel::drivers::i6300esb::hardware::discover::' in line
                 or ('kernel::drivers::i6300esb::Watchdog' in line and '::new::' in line)]
        self.assertGreaterEqual(len(paths), 2, 'expected feeding and discovery paths')
        for index in paths:
            start = int(symbols[index].split()[0], 16)
            end = next(int(line.split()[0], 16) for line in symbols[index + 1:]
                       if int(line.split()[0], 16) > start)
            assembly = subprocess.check_output([objdump, '-d', '--start-address=' + hex(start),
                                                '--stop-address=' + hex(end), str(kernel)], text=True)
            # HVF requires ISV=1 for emulated device reads and writes. Stack
            # frame writeback accesses ordinary RAM and is excluded here.
            accesses = [line for line in assembly.splitlines()
                        if re.search(r'\b(?:ldr|str)(?:b|h)?\s', line) and '[sp' not in line]
            self.assertTrue(accesses, assembly)
            self.assertFalse(any('!' in line or re.search(r'\],\s*#', line) for line in accesses), assembly)


if __name__ == '__main__':
    unittest.main()
