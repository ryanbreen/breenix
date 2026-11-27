#!/usr/bin/env python3
"""Build both arches with build.sh --rust-host before running this check."""
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location('rust_host_stage', ROOT / 'scripts/rust_host_stage.py')
STAGE = importlib.util.module_from_spec(SPEC)

HOSTED = ROOT / 'userspace/programs/target/hosted'
READELF = os.environ.get('LLVM_READELF', 'llvm-readelf')


class StartupProbeTest(unittest.TestCase):
    def test_arm_rust_host_build_matches_sequence_elf(self):
        build = ROOT / 'userspace/programs/build.sh'
        for args in (('--arch', 'aarch64'), ('--arch', 'aarch64', '--rust-host')):
            result = subprocess.run([build, *args], cwd=ROOT, stdout=subprocess.PIPE,
                                    stderr=subprocess.STDOUT, text=True)
            self.assertEqual(result.returncode, 0, result.stdout)
            self.assertIsNone(re.search(r'(?im)^\s*warning(?:\[|:)', result.stdout), result.stdout)

        release = HOSTED / 'aarch64-breenix-hosted/release'
        suite = release / 'suite-rust-host'
        self.assertEqual((ROOT / 'userspace/programs/aarch64/suite-rust-host.elf').read_bytes(),
                         suite.read_bytes())
        x86_release = HOSTED / 'x86_64-breenix-hosted/release'
        for name in ('suite-rust-host', 'rust-host-startup-probe', 'rust-host-process-probe',
                     'rust-host-unwind-probe'):
            with self.subTest(elf=name):
                arm_elf = release / name
                x86_elf = x86_release / name
                arm_machine, arm_headers = self.elf_headers(arm_elf)
                x86_machine, x86_headers = self.elf_headers(x86_elf)
                self.assertEqual(arm_machine, 183)
                self.assertEqual(x86_machine, 62)
                self.assertIn(0x6474E550, arm_headers)  # PT_GNU_EH_FRAME
                self.assertIn(0x6474E551, arm_headers)  # PT_GNU_STACK

                # Ruling 146(2), ELF gABI §7.1: p_align=0 denotes no alignment requirement.
                arm_tls = arm_headers.get(7)  # PT_TLS
                x86_tls = x86_headers.get(7)
                self.assertEqual((arm_tls or {}).get('MemSize', 0) > 0,
                                 (x86_tls or {}).get('MemSize', 0) > 0,
                                 'ARM and x86 must agree on whether the TLS template is non-empty')
                for elf, tls in ((arm_elf, arm_tls), (x86_elf, x86_tls)):
                    with self.subTest(elf=elf):
                        if name == 'rust-host-startup-probe':
                            self.assertIsNotNone(tls)
                            self.assertGreater(tls['MemSize'], 0)
                        if tls is not None:
                            self.assertGreaterEqual(tls['MemSize'], tls['FileSize'])
                            align = tls['Alignment']
                            self.assertTrue(align == 0 or (align > 0 and (align & (align - 1)) == 0),
                                            f'PT_TLS p_align={align} must be zero or a power of two')
                            self.assertLessEqual(tls['Offset'] + tls['FileSize'], elf.stat().st_size)

    def elf_headers(self, elf):
        [report] = json.loads(subprocess.check_output([
            READELF, '--elf-output-style=JSON', '--file-header', '--program-headers', elf,
        ]))
        headers = {row['ProgramHeader']['Type']['Value']: row['ProgramHeader']
                   for row in report['ProgramHeaders']}
        return report['ElfHeader']['Machine']['Value'], headers

    def test_unwind_personality_slot_is_readable_by_bunwind(self):
        for arch in ('x86_64', 'aarch64'):
            with self.subTest(arch=arch):
                elf = HOSTED / f'{arch}-breenix-hosted/release/rust-host-unwind-probe'
                [report] = json.loads(subprocess.check_output([
                    READELF, '--elf-output-style=JSON', '--program-headers', '--symbols', elf,
                ]))
                symbols = {row['Symbol']['Name']['Name']: row['Symbol'] for row in report['Symbols']}
                slot = symbols['DW.ref.rust_eh_personality']
                self.assertEqual(slot['Size'], 8)
                loads = [row['ProgramHeader'] for row in report['ProgramHeaders']
                         if row['ProgramHeader']['Type']['Value'] == 1]
                # LSB Core 5.0 §10.3.1.2: the indirect encoding reads an
                # address from the slot. Bunwind reads file-backed R loads.
                readable = [load for load in loads if load['Flags']['Value'] == 4
                            and 0 <= slot['Value'] - load['VirtualAddress'] <= load['FileSize'] - slot['Size']]
                self.assertEqual(len(readable), 1, 'personality slot must be in a read-only load')
                load = readable[0]
                offset = load['Offset'] + slot['Value'] - load['VirtualAddress']
                pointer = elf.read_bytes()[offset:offset + slot['Size']]
                self.assertEqual(int.from_bytes(pointer, 'little'), symbols['rust_eh_personality']['Value'])

    def test_file_mmap_fixture_is_staged_read_only(self):
        SPEC.loader.exec_module(STAGE)
        debugfs, mke2fs = STAGE.require_e2fsprogs('debugfs', 'mke2fs')
        path = '/usr/libexec/breenix/rust-host/mmap-fixture'
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            image = root / 'root.img'
            elf = root / 'probe'
            elf.write_bytes(b'\x7fELF')
            subprocess.run([mke2fs, '-q', '-t', 'ext2', '-F', image, '16M'], check=True, capture_output=True)
            STAGE.stage(image, 'rust-host', elf, elf, elf, elf)

            stat = subprocess.check_output([debugfs, '-R', f'stat {path}', image], stderr=subprocess.DEVNULL)
            self.assertIn(b'Mode:  0444', stat)
            dump = root / 'fixture'
            subprocess.run([debugfs, '-R', f'dump {path} {dump}', image], check=True, capture_output=True)
            self.assertEqual(dump.stat().st_size, 163_067_344)
            # The original guest pattern encodes each word's file offset as little-endian u64.
            with dump.open('rb') as stream:
                self.assertEqual(
                    hashlib.file_digest(stream, 'sha256').hexdigest(),
                    '641a09f91a4a48c8fad03a45411ab8e714f1612914175abae70e74d09b92427f',
                )

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
        self.assertIn('HOSTED_BIN="rust-host-unwind-probe"', build)
        self.assertIn('linker-hosted.ld', build)
        self.assertIn('--eh-frame-hdr', build)

    def test_built_elf_and_staged_image(self):
        for arch, machine in [('x86_64', 62), ('aarch64', 183)]:
            with self.subTest(arch=arch):
                release = HOSTED / f'{arch}-breenix-hosted/release'
                suite = release / 'suite-rust-host'
                probe = release / 'rust-host-startup-probe'
                process_probe = release / 'rust-host-process-probe'
                unwind_probe = release / 'rust-host-unwind-probe'
                for elf in (probe, suite, process_probe, unwind_probe):
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
                        STAGE.stage(image, selection, suite, probe, process_probe, unwind_probe)
                        self.assertEqual(image.read_bytes(), original)
                    STAGE.stage(image, 'time,rust-host', suite, probe, process_probe, unwind_probe)
                    for path, elf in [('/sbin/suite-rust-host', suite),
                                      ('/usr/libexec/breenix/rust-host/startup-probe', probe),
                                      ('/usr/libexec/breenix/rust-host/process-probe', process_probe),
                                      ('/usr/libexec/breenix/rust-host/unwind-probe', unwind_probe)]:
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
