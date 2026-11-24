#!/usr/bin/env python3
"""Build both arches with build.sh --rust-host before running this check."""
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location('rust_host_stage', ROOT / 'scripts/rust_host_stage.py')
STAGE = importlib.util.module_from_spec(SPEC)

HOSTED = ROOT / 'userspace/programs/target/hosted'
READELF = os.environ.get('LLVM_READELF', 'llvm-readelf')


class StartupProbeTest(unittest.TestCase):
    def test_selection_and_hosted_layout(self):
        SPEC.loader.exec_module(STAGE)
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            probe = root / 'startup-probe'
            probe.write_bytes(b'hosted fixture')
            for selection, expected in [('time', False), ('rust-host', True), ('time,rust-host', True)]:
                self.assertEqual(STAGE.probe_for_suite(selection, probe), probe if expected else None)
        build = (ROOT / 'userspace/programs/build.sh').read_text()
        self.assertIn('HOSTED_BIN="rust-host-startup-probe"', build)
        self.assertIn('linker-hosted.ld', build)
        self.assertIn('--eh-frame-hdr', build)

    def test_built_elf_and_staged_image(self):
        for arch, machine in [('x86_64', 62), ('aarch64', 183)]:
            with self.subTest(arch=arch):
                release = HOSTED / f'{arch}-breenix/release'
                suite = release / 'suite-rust-host'
                probe = release / 'rust-host-startup-probe'
                process_probe = release / 'rust-host-process-probe'
                for elf in (probe, suite, process_probe):
                    self.assertTrue(elf.is_file(), f'missing hosted build output: {elf}')
                    [report] = json.loads(subprocess.check_output([
                        READELF, '--elf-output-style=JSON', '--file-header', '--program-headers',
                        '--sections', '--symbols', elf,
                    ]))
                    self.assertEqual(report['ElfHeader']['Machine']['Value'], machine)
                    sections = {row['Section']['Name']['Name']: row['Section'] for row in report['Sections']}
                    headers = {row['ProgramHeader']['Type']['Value']: row['ProgramHeader']
                               for row in report['ProgramHeaders']}
                    # LSB Core 5.0 Table 11-1 and linker-hosted.ld map the EH index,
                    # and mark the process stack as read/write (gABI Table 7.3).
                    eh = headers[0x6474E550]
                    index = sections['.eh_frame_hdr']
                    self.assertGreater(index['Size'], 0)
                    self.assertEqual((eh['VirtualAddress'], eh['FileSize']), (index['Address'], index['Size']))
                    self.assertEqual(headers[0x6474E551]['Flags']['Value'], 6)
                    if elf == probe:
                        self.assert_probe_arrays(report, sections, headers)

                SPEC.loader.exec_module(STAGE)
                debugfs, mke2fs = STAGE.require_e2fsprogs('debugfs', 'mke2fs')
                with tempfile.TemporaryDirectory() as directory:
                    image = Path(directory) / 'root.img'
                    subprocess.run([mke2fs, '-q', '-t', 'ext2', '-F', image, '16M'], check=True, capture_output=True)
                    original = image.read_bytes()
                    for selection in ('', 'time', 'all', 'rust-host-other'):
                        STAGE.stage(image, selection, suite, probe, process_probe)
                        self.assertEqual(image.read_bytes(), original)
                    STAGE.stage(image, 'time,rust-host', suite, probe, process_probe)
                    for path, elf in [('/sbin/suite-rust-host', suite),
                                      ('/usr/libexec/breenix/rust-host/startup-probe', probe),
                                      ('/usr/libexec/breenix/rust-host/process-probe', process_probe)]:
                        self.assertEqual(subprocess.check_output(
                            [debugfs, '-R', f'cat {path}', image], stderr=subprocess.DEVNULL,
                        ), elf.read_bytes())
                        self.assertIn(b'Mode:  0755', subprocess.check_output(
                            [debugfs, '-R', f'stat {path}', image], stderr=subprocess.DEVNULL,
                        ))

    def assert_probe_arrays(self, report, sections, headers):
        self.assertIn(1, headers)  # ELF gABI Table 7.2: PT_LOAD.
        self.assertGreater(headers[7]['MemSize'], 0)  # PT_TLS.
        symbols = {row['Symbol']['Name']['Name']: row['Symbol']['Value'] for row in report['Symbols']}
        for phase, kind in [('preinit', 16), ('init', 14), ('fini', 15)]:
            # gABI Tables 3.2 and 3.8 and linker-hosted.ld's KEEP and CRT aliases.
            array = sections[f'.{phase}_array']
            self.assertEqual((array['Type']['Value'], array['Size']), (kind, 16))
            self.assertEqual(symbols[f'__breenix_{phase}_array_start'], array['Address'])
            self.assertEqual(symbols[f'__breenix_{phase}_array_end'], array['Address'] + array['Size'])


if __name__ == '__main__':
    unittest.main()
