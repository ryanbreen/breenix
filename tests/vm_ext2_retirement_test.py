"""Exercise disposable VM upgrade and GPT retirement with retained boot receipts."""
import json
from pathlib import Path
import runpy
import struct
import subprocess
import tempfile
import unittest
import uuid
import zlib

ROOT = Path(__file__).resolve().parents[1]
VM = runpy.run_path(str(ROOT / 'scripts/vm-artifacts.py'))
MOUNTS = ('none / breenfs ro 0 0\nnone /home breenfs rw,nosuid,nodev 0 0\n' +
          ''.join(f'none {path} breenfs rw 0 0\n' for path in VM['SHARED_BINDS']))
SENTINEL = 'a' * 64
HOST_KEY = 'b' * 64
RETURN = '20261121T200000Z-qemu'
INSTALL_LINES = [
    '[boot] ext2 root filesystem mounted',
    '[persistent] install starting: the test root\'s userland onto BreenFS',
    '[persistent] recovered slot references needed',
    '[persistent] layout set-ID check passed',
    '[persistent] install complete: files=10 dirs=4 links=2 hard-links=1 bytes=100 left-out=0',
]


class VmExt2Retirement(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.repo, self.run = Path(directory.name) / 'repo', Path(directory.name) / 'run'
        self.repo.mkdir()
        self.run.mkdir()
        git = ['git', '-C', str(self.repo)]
        subprocess.run([*git, 'init', '-q'], check=True)
        subprocess.run([*git, '-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid',
                        'commit', '--allow-empty', '-qm', 'fixture'], check=True)
        self.commit = subprocess.check_output([*git, 'rev-parse', 'HEAD'], text=True).strip()
        self.guids = {role: str(uuid.uuid4()) for role in
                      ('test-root', 'system', 'home', 'breenfs-system', 'breenfs-home')}
        for role in self.guids:
            (self.run / f'{role}.img').write_bytes(role.encode().ljust(512, b'\x5a'))
        (self.run / 'kernel').write_bytes(b'fixture kernel')
        (self.run / 'power-token').write_text('c' * 64)
        (self.run / '.breenix-vm-persistent').touch()
        self.write('volume-guids.json', self.guids)
        VM['seal'](self.repo, self.run, self.commit)
        VM['seal_slot'](self.repo, self.run, self.run / 'kernel')
        entry = dict(kernel='kernel', kernelSha256=VM['digest'](self.run / 'kernel'), userland='base')
        self.write('slots.json', dict(current=dict(entry, layout='breenfs'),
                                      previous=dict(entry, layout='ext2-rescue'), next=None))
        ext2 = VM['disk_digests'](self.run, VM['EXT2_DISKS'])
        disks = VM['disk_digests'](self.run, VM['MIGRATION_DISKS'])
        self.write('migration-cycle.json', dict(phase='return', userland='base', ext2=ext2,
                                                disks=disks, compared=disks,
                                                boots={'migrate': '20261121T195600Z-qemu',
                                                       'cutover': '20261121T195700Z-qemu',
                                                       'rescue': '20261121T195800Z-qemu', 'return': RETURN}))
        self.boot(RETURN, 'return', 'base')

    def write(self, name, record):
        (self.run / name).write_text(json.dumps(record) + '\n')

    def boot(self, ident, phase, generation, pending=None, probe=True):
        boot = self.run / 'boots' / ident
        boot.mkdir(parents=True)
        (self.run / 'current').write_text(ident + '\n')
        (boot / 'ended').touch()
        record = dict(id=ident, target='qemu', bootedCommit=self.commit, toolsCommit=self.commit)
        if pending is not None:
            record['retirement'] = pending
        self.write(boot.relative_to(self.run) / 'run.json', record)
        if phase == 'install':
            lines = INSTALL_LINES
            events = [dict(event='SHUTDOWN', data=dict(reason='host-qmp-quit'))]
        else:
            lines = ['[boot] breenfs root filesystem mounted', 'bsshd: listening on 0.0.0.0:22']
            events = [dict(event='requested-shutdown', action='poweroff'),
                      dict(event='SHUTDOWN', data=dict(reason='guest-shutdown'))]
            if probe:
                self.write(boot.relative_to(self.run) / VM['MIGRATION_PROBE'],
                           dict(boot=ident, phase=phase, generation=generation, mounts=MOUNTS,
                                hostKeySha256=HOST_KEY, sentinelSha256=SENTINEL))
        (boot / 'serial.txt').write_text('\n'.join(lines) + '\n')
        (boot / 'events.jsonl').write_text(''.join(json.dumps(row) + '\n' for row in events))
        return boot

    def begin(self):
        VM['begin_retirement'](self.repo, self.run)
        return self.run / 'retirement.img'

    def launch(self, index, phase, slot, *, probe=True):
        VM['prepare_retirement'](self.repo, self.run, 'install' if phase == 'install' else 'boot', slot)
        pending = json.loads((self.run / 'verified-build.json').read_text())['retirement']
        ident = f'20261121T2000{index:02d}Z-qemu'
        self.boot(ident, phase, pending['generation'], pending, probe)
        VM['finish_retirement'](self.run, 0, 0)
        return ident

    def upgrade_twice(self):
        for index in (1, 3):
            self.launch(index, 'install', 'current')
            self.launch(index + 1, 'verify', 'next')

    def test_upgrade_twice_then_reclaim_and_verify_current(self):
        disk = self.begin()
        before = disk.read_bytes()
        self.upgrade_twice()
        VM['reclaim_retirement'](self.repo, self.run)
        after = disk.read_bytes()
        self.assertEqual(len(before), len(after))
        expected = bytearray(before)
        array_locations = []
        for at in (512, len(before) - 512):
            header = bytearray(expected[at:at + 512])
            self.assertEqual(header[:8], b'EFI PART')
            array = struct.unpack_from('<Q', header, 72)[0] * 512
            array_locations.append(array)
            for number, role in ((3, 'system'), (4, 'home')):
                entry = array + (number - 1) * 128
                self.assertEqual(before[entry + 16:entry + 32], uuid.UUID(self.guids[role]).bytes_le)
                expected[entry:entry + 128] = bytes(128)
            struct.pack_into('<I', header, 88, zlib.crc32(expected[array:array + 128 * 128]))
            struct.pack_into('<I', header, 16, 0)
            struct.pack_into('<I', header, 16, zlib.crc32(header[:92]))
            expected[at:at + 512] = header
        self.assertEqual(len(set(array_locations)), 2)
        self.assertTrue(expected == after, 'reclaim changed bytes outside entries 3 and 4 and their CRCs')
        final = self.launch(5, 'final', 'current')
        proof = json.loads((self.run / 'filesystem-reclaimed.json').read_text())
        self.assertEqual(proof['boot'], final)
        self.assertEqual(proof['nextAnswer']['slot'], 'current')
        self.assertEqual(proof['nextAnswer']['userland'], proof['upgrades'][-1]['generation'])
        self.assertEqual([row['result'] for row in proof['cases']], ['PASS'])
        self.assertEqual([row['result'] for row in
                          json.loads((self.run / 'filesystem-two-upgrades.json').read_text())['cases']], ['PASS'])

    def test_failed_candidate_never_reclaims(self):
        disk = self.begin()
        old = VM['digest'](disk)
        self.launch(1, 'install', 'current')
        with self.assertRaisesRegex(ValueError, 'verification receipts'):
            self.launch(2, 'verify', 'next', probe=False)
        with self.assertRaisesRegex(ValueError, 'two verified upgrades'):
            VM['reclaim_retirement'](self.repo, self.run)
        self.assertEqual(VM['digest'](disk), old)
        self.assertEqual(json.loads((self.run / 'slots.json').read_text())['current']['userland'], 'base')
        self.assertFalse((self.run / 'filesystem-reclaimed.json').exists())

    def test_disagreeing_gpt_refuses_before_any_write(self):
        disk = self.begin()
        self.upgrade_twice()
        with disk.open('r+b') as output:
            output.seek(-512 - 128 * 128 + 56, 2)
            old = output.read(1)
            output.seek(-1, 1)
            output.write(bytes([old[0] ^ 1]))
        damaged = VM['digest'](disk)
        with self.assertRaisesRegex(ValueError, 'GPT'):
            VM['reclaim_retirement'](self.repo, self.run)
        self.assertEqual(VM['digest'](disk), damaged)
        self.assertFalse((self.run / 'filesystem-reclaimed.json').exists())


if __name__ == '__main__':
    unittest.main()
