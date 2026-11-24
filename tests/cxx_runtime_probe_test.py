"""Verify the artifacts produced by build.sh --cxx-runtime on both targets."""

import json
import os
from pathlib import Path
import re
import subprocess
import unittest

ROOT = Path(__file__).resolve().parents[1]
PROGRAMS = ROOT / 'userspace/programs'
LLVM_BIN = Path('/opt/homebrew/opt/llvm/bin')
PROBES = ('static-init-order', 'guarded-static', 'containers-threads')
MACHINES = {'x86_64': 'EM_X86_64', 'aarch64': 'EM_AARCH64'}


class CxxRuntimeProbeTest(unittest.TestCase):
    def test_probes_are_static_hosted_elfs(self):
        readelf = os.environ.get('LLVM_READELF', LLVM_BIN / 'llvm-readelf')
        nm = os.environ.get('LLVM_NM', LLVM_BIN / 'llvm-nm')
        for arch, machine in MACHINES.items():
            directory = PROGRAMS / 'target/cxx-runtime' / arch
            for probe in PROBES:
                with self.subTest(arch=arch, probe=probe):
                    elf = directory / probe
                    self.assertTrue(elf.is_file(), f'missing {elf}; run build.sh --arch {arch} --cxx-runtime')
                    [report] = json.loads(subprocess.check_output(
                        [readelf, '--elf-output-style=JSON', '--file-header', '--program-headers', elf],
                        timeout=5,
                    ))
                    # ELF gABI chapter 2, table 2.1: ET_EXEC has e_type 2.
                    self.assertEqual(report['ElfHeader']['Type'], 'Executable (0x2)')
                    self.assertEqual(report['ElfHeader']['Machine']['Name'], machine)
                    segments = {
                        row['ProgramHeader']['Type']['Name'] for row in report['ProgramHeaders']
                    }
                    # Hosted PHDRS are declared in userspace/programs/linker-hosted.ld.
                    self.assertNotIn('PT_INTERP', segments)
                    self.assertLessEqual({'PT_GNU_EH_FRAME', 'PT_GNU_STACK'}, segments)
                    dynamic = subprocess.check_output(
                        [readelf, '--dynamic-table', elf], text=True, timeout=5,
                    )
                    self.assertIsNone(re.search(r'\bNEEDED\b', dynamic), dynamic)
                    undefined = subprocess.check_output(
                        [nm, '--undefined-only', elf], text=True, timeout=5,
                    )
                    self.assertEqual(undefined, '')
                    link_map = (directory / f'{probe}.map').read_text()
                    self.assertIn('Linker script: userspace/programs/linker-hosted.ld',
                                  link_map.splitlines())


if __name__ == '__main__':
    unittest.main()
