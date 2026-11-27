"""Verify the artifacts produced by build.sh --cxx-runtime on both targets."""

import json
import importlib.util
import tempfile
import os
from pathlib import Path
import re
import shlex
import shutil
import subprocess
import sys
import unittest

ROOT = Path(__file__).resolve().parents[1]
PROGRAMS = ROOT / 'userspace/programs'
LLVM_BIN = Path('/opt/homebrew/opt/llvm/bin')
PROBES = ('static-init-order', 'guarded-static', 'containers-threads')
MACHINES = {'x86_64': 'EM_X86_64', 'aarch64': 'EM_AARCH64'}


class CxxRuntimeProbeTest(unittest.TestCase):
    def test_containers_probe_rejects_no_op_mutex(self):
        source = PROGRAMS / 'cxx/containers-threads.cpp'
        expected = json.loads((source.parent / 'expect.json').read_text())['containers-threads']
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            mutant = root / source.name
            shim = '''struct NoOpMutex {
    void lock() {}
    bool try_lock() { return true; }
    void unlock() {}
};
'''
            original = source.read_text()
            self.assertIn('std::mutex mutex;', original)
            mutant.write_text(shim + original.replace('std::mutex mutex;', 'NoOpMutex mutex;'))
            compiler = shlex.split(os.environ.get('CXX', 'c++'))
            for implementation in (source, mutant):
                binary = root / 'probe'
                subprocess.run(
                    [*compiler, '-std=c++23', '-O2', '-pthread', '-Wall', '-Wextra', '-Werror',
                     '-I', source.parent, implementation, '-o', binary], check=True, timeout=30)
                result = subprocess.run([binary, '1'], capture_output=True, text=True, timeout=5)
                if implementation == source:
                    self.assertEqual((result.returncode, result.stdout), (0, expected))
                else:
                    self.assertEqual(result.returncode, 1, result.stdout + result.stderr)

    def test_arm_cxx_runtime_image_ready(self):
        spec = importlib.util.spec_from_file_location('rust_host_stage', ROOT / 'scripts/rust_host_stage.py')
        stage = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(stage)
        debugfs, = stage.require_e2fsprogs('debugfs')
        seed = ROOT / 'target/ext2-aarch64.img'
        hosted = PROGRAMS / 'target/hosted/aarch64-breenix-hosted/release/suite-cxx-runtime'
        probes = PROGRAMS / 'target/cxx-runtime/aarch64'
        self.assertTrue(seed.is_file(), 'run scripts/create_ext2_disk.sh --arch aarch64 first')

        with tempfile.TemporaryDirectory() as directory:
            image = Path(directory) / 'cxx-runtime.img'
            shutil.copyfile(seed, image)
            for command in (
                [sys.executable, ROOT / 'scripts/rust_host_stage.py', '--arch', 'aarch64',
                 '--image', image, '--suite', 'cxx-runtime'],
                [ROOT / 'scripts/write-boot-target.sh', image, 'cxx-runtime',
                 PROGRAMS / 'aarch64/suite-cxx-runtime.elf', '', '', '', 'ext2', hosted],
            ):
                result = subprocess.run(command, cwd=ROOT, capture_output=True, text=True, timeout=30)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

            def read_image(command):
                return subprocess.check_output([debugfs, '-R', command, image],
                                               stderr=subprocess.DEVNULL, timeout=10)

            self.assertEqual(read_image('cat /etc/breenix/boot-target'), b'suite cxx-runtime\n')
            self.assertEqual(read_image('cat /sbin/suite-cxx-runtime'), hosted.read_bytes())
            for name in PROBES:
                with self.subTest(probe=name):
                    path = f'/usr/libexec/breenix/cxx-runtime/{name}'
                    self.assertEqual(read_image(f'cat {path}'), (probes / name).read_bytes())
                    self.assertIn(b'Type: regular    Mode:  0755', read_image(f'stat {path}'))
            for name in ('startup-probe', 'process-probe', 'unwind-probe', 'mmap-fixture'):
                with self.subTest(rust_host=name):
                    self.assertNotIn(b'Inode:', read_image(f'stat /usr/libexec/breenix/rust-host/{name}'))

    def test_probes_staged_only_under_cxx_runtime(self):
        spec = importlib.util.spec_from_file_location('rust_host_stage', ROOT / 'scripts/rust_host_stage.py')
        stage = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(stage)
        debugfs, mke2fs = stage.require_e2fsprogs('debugfs', 'mke2fs')
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            image = root / 'root.img'
            suite = root / 'suite'
            suite.write_bytes(b'\x7fELFsuite')
            probes = []
            for name in PROBES:
                probe = root / name
                probe.write_bytes(b'\x7fELF' + name.encode())
                probes.append(probe)
            subprocess.run([mke2fs, '-q', '-t', 'ext2', '-F', image, '16M'], check=True, capture_output=True)
            original = image.read_bytes()
            for selection in ('', 'all', 'rust-host', 'cxx-runtime-other'):
                stage.stage_cxx_runtime(image, selection, suite, probes)
                self.assertEqual(image.read_bytes(), original)
            stage.stage_cxx_runtime(image, 'time,cxx-runtime', suite, probes)
            for path, elf in [('/sbin/suite-cxx-runtime', suite), *[
                (f'/usr/libexec/breenix/cxx-runtime/{name}', probe)
                for name, probe in zip(PROBES, probes)
            ]]:
                self.assertEqual(subprocess.check_output([debugfs, '-R', f'cat {path}', image],
                                                        stderr=subprocess.DEVNULL), elf.read_bytes())
                self.assertIn(b'Mode:  0755', subprocess.check_output(
                    [debugfs, '-R', f'stat {path}', image], stderr=subprocess.DEVNULL))

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
