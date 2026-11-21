"""The R240 filesystem suite must receive the same disks as ARM64 QEMU."""
from pathlib import Path
import re
import sys
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
import metal_tools_test as existing
import metal_disk as disk

persist = disk.metal_persist
REPO = Path(__file__).resolve().parents[1]
MIB = 1 << 20
ROOT_BYTES = 4096
FIXTURE_BYTES = 64 * MIB


class FilesystemFixturesTest(existing.StateTest):
    def setUp(self):
        super().setUp()
        self.directory = existing.m.STATE / 'boots' / 'suite'
        self.inputs = self.directory / 'inputs'
        self.inputs.mkdir(parents=True)
        image = bytearray(ROOT_BYTES)
        image[1080:1082] = b'\x53\xef'
        (self.inputs / 'ext2.img').write_bytes(image)
        self.build = {'ext2': {'sha256': disk.metal_image.sha256(self.inputs / 'ext2.img')}}
        self.site = {}

    def test_fixture_payload_and_intent_follow_arm_discovery_order(self):
        launcher = (REPO / 'scripts/boot-interactive.sh').read_text()
        scratch = (REPO / 'scripts/lib/root-fs.sh').read_text()
        groups = {'BREENFS_SCRATCH_ARGS': scratch, 'BREENFS_CORRUPT_ARGS': launcher,
                  'BREENFS_READ_ARGS': launcher}
        attachments = []
        for group in re.findall(r'^    \$\{(BREENFS_[A-Z_]+)\[@\]\+', launcher, re.M):
            assignment = groups[group].split(group + '=(-device ', 1)[1].split(')\n', 1)[0]
            attachments.extend(re.findall(r'drive=breenfs-([\w-]+)', assignment))
        self.assertEqual(attachments, ['scratch', 'corrupt-data', 'corrupt-node', 'read'])
        roles = list(reversed(attachments))  # ARM allocates MMIO buses downward.
        built = {}
        for role in roles:
            name = f'breenfs-{role}.img'
            path = self.inputs / name
            with path.open('wb') as image:
                image.write(role.encode())
                image.truncate(FIXTURE_BYTES)
            built[name] = dict(size=FIXTURE_BYTES, sha256=disk.metal_image.sha256(path))
        self.build['filesystemFixtures'] = built
        layout = persist.new_layout()
        with patch.object(disk.metal_root, 'stage_root'), patch.object(disk.metal_root, 'console_identity', return_value='fixture'):
            facts, files = disk.stage(self.directory, self.inputs, 'suite', 'b' * 40, self.site,
                                     self.build, mode='suite', suite='directories,filesystem,files-io',
                                     the_layout=layout)
        self.assertEqual(facts['bytes'], ROOT_BYTES)
        slices = facts.get('fixtureSlices', [])
        expected = [dict(role=role, offset=ROOT_BYTES + index * FIXTURE_BYTES, length=FIXTURE_BYTES)
                    for index, role in enumerate(roles)]
        self.assertEqual(slices, expected)
        intent = persist.intent('suite directories,filesystem,files-io', root=facts['partition'],
                                fixtures=slices)
        self.assertEqual([line for line in intent.decode().splitlines() if line.startswith('fixture ')],
                         [f"fixture {s['offset']} {s['length']} {s['role']}" for s in expected])
        self.assertEqual(disk.HEADER_2.unpack(files['suite-disk.cfg'].read_bytes())[1:3],
                         (1, ROOT_BYTES + 4 * FIXTURE_BYTES))
        with files['suite-root.img'].open('rb') as payload:
            self.assertEqual(payload.read(512), disk.record(facts))
            for item in slices:
                payload.seek(item['offset'])
                self.assertEqual(payload.read(len(item['role'])), item['role'].encode())
        # Model a stopped guest write. Scoring must use those bytes, from this
        # capture, and pass the root image at its original length.
        cold = files['suite-root.img']
        with cold.open('r+b') as captured:
            captured.seek(slices[-1]['offset'])
            captured.write(b'stopped')
        root = disk.unpack_readback(cold, facts['bytes'], slices)
        self.assertEqual(root.stat().st_size, ROOT_BYTES)
        scratch = root.with_name('breenfs-scratch.img')
        self.assertEqual(scratch.stat().st_size, FIXTURE_BYTES)
        with scratch.open('rb') as image:
            self.assertEqual(image.read(7), b'stopped')

    def test_reject_fixture_ranges_before_staging(self):
        cap = ROOT_BYTES + 4 * FIXTURE_BYTES
        items = [dict(role=role, offset=ROOT_BYTES + index * FIXTURE_BYTES, length=FIXTURE_BYTES)
                 for index, role in enumerate(('read', 'corrupt-node', 'corrupt-data', 'scratch'))]
        self.assertEqual(disk.validate_fixture_slices(ROOT_BYTES, cap, items), cap)
        with self.assertRaises(ValueError):
            disk.validate_fixture_slices(ROOT_BYTES, cap - 512, items)
        for index, offset in ((0, ROOT_BYTES - 512), (1, items[1]['offset'] - 512),
                              (3, (1 << 64) - 512)):
            with self.subTest(index=index, offset=offset):
                bad = [dict(item) for item in items]
                bad[index]['offset'] = offset
                with self.assertRaises(ValueError):
                    disk.validate_fixture_slices(ROOT_BYTES, cap, bad)
