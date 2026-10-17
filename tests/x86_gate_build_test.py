#!/usr/bin/env python3
"""The build container's reply, as the gate VM installs it, without a container or ssh."""
import importlib.util
import io
import json
from pathlib import Path
import tarfile
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('x86_gate_build', ROOT / 'scripts/x86-gate-build.py')
build = importlib.util.module_from_spec(spec)
spec.loader.exec_module(build)
MIB = 1 << 20


class TransferTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.container, self.vm, self.logs = self.root / 'container', self.root / 'vm', self.root / 'vm-logs'
        (self.container / 'target/release').mkdir(parents=True)
        (self.container / 'testdata').mkdir()
        launcher = self.container / 'target/release/qemu-uefi'
        launcher.write_bytes(b'launcher')
        launcher.chmod(0o755)
        with (self.container / 'testdata/ext2.img').open('wb') as disk:
            disk.truncate(8 * MIB)
            disk.seek(3 * MIB)
            disk.write(b'superblock')
        self.build_logs = self.root / 'gate-tmp'
        (self.build_logs / 'build-logs.abc').mkdir(parents=True)
        (self.build_logs / 'build-logs.abc/gate-build.log').write_text('Finished release\n')
        self.files = {'target/release/qemu-uefi': 8, 'testdata/ext2.img': 8 * MIB}

    def reply(self, files=None, status=0):
        stream = io.BytesIO()
        build.send(stream, self.build_logs, self.container, self.files if files is None else files,
                   {'status': status, 'files': self.files})
        return stream.getvalue()

    def test_outputs_logs_and_modes_survive_the_transfer(self):
        status = build.receive(io.BytesIO(self.reply()), self.vm, self.logs)
        self.assertEqual(status['status'], 0)
        for name in self.files:
            self.assertEqual((self.vm / name).read_bytes(), (self.container / name).read_bytes())
        self.assertTrue((self.vm / 'target/release/qemu-uefi').stat().st_mode & 0o100)
        self.assertEqual((self.logs / 'build-logs.abc/gate-build.log').read_text(), 'Finished release\n')

    def test_an_images_holes_are_skipped_not_written(self):
        class Recording(io.BytesIO):
            written = 0

            def write(self, data):
                self.written += len(data)
                return super().write(data)
        image = bytes(3 * MIB) + b'superblock'.ljust(MIB, b'\0') + bytes(4 * MIB)
        output = Recording()
        build.copy_sparse(io.BytesIO(image), output)
        self.assertEqual(output.written, MIB)
        # A file's truncate() would extend it over the trailing hole; BytesIO's only shrinks.
        self.assertEqual(output.getvalue().ljust(len(image), b'\0'), image)

    def test_a_reply_that_ends_before_its_status_is_refused(self):
        data = self.reply()
        with self.assertRaises((tarfile.TarError, RuntimeError)):
            build.receive(io.BytesIO(data[:len(data) // 2]), self.vm, self.logs)

    def test_a_listed_output_that_did_not_arrive_is_refused(self):
        data = self.reply(files={'target/release/qemu-uefi': 8})
        with self.assertRaisesRegex(RuntimeError, 'testdata/ext2.img did not arrive'):
            build.receive(io.BytesIO(data), self.vm, self.logs)

    def test_a_member_outside_the_tree_is_refused(self):
        stream = io.BytesIO()
        with tarfile.open(fileobj=stream, mode='w') as archive:
            data = b'x'
            info = tarfile.TarInfo('tree/../../escaped')
            info.size = len(data)
            archive.addfile(info, io.BytesIO(data))
        with self.assertRaisesRegex(RuntimeError, 'unsafe path'):
            build.receive(io.BytesIO(stream.getvalue()), self.vm, self.logs)
        self.assertFalse((self.root / 'escaped').exists())


if __name__ == '__main__':
    unittest.main()
