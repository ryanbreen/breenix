"""Physical-root delivery and cold raw readback; no controller is contacted."""
import hashlib
import json
from pathlib import Path
import sys
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
import metal_tools_test as existing
import metal_disk as disk
import metal_root


class DiskTest(existing.StateTest):
    def setUp(self):
        super().setUp()
        self.directory = existing.m.STATE / 'boots' / 'suite'
        self.directory.mkdir(parents=True)
        self.inputs = self.directory / 'inputs'
        self.inputs.mkdir()
        image = bytearray(4096)
        image[1080:1082] = b'\x53\xef'
        (self.inputs / 'ext2.img').write_bytes(image)
        (self.inputs / 'stub.efi').write_bytes(b'stub')
        self.build = {'ext2': {'sha256': hashlib.sha256(image).hexdigest()}}
        self.site = {'CONSOLE_USER': 'root', 'CONSOLE_PASSWORD': 'fixture_password_only_12345',
                     'CONSOLE_IDENTITY_KEY': 'ab' * 32}
        existing.m.write_json(existing.m.STATE / 'disk.json',
            dict(installedBy='prior', bytes=4096, sha256AfterRecord='a' * 64))

    def stage(self, boot='first'):
        return disk.stage(self.directory, self.inputs, boot, 'b' * 40, self.site, self.build)

    def test_changed_build_installs_but_same_userspace_across_commits_reuses_disk(self):
        with patch.object(metal_root, 'stage_root') as root:
            facts, files = self.stage()
            root.assert_called_once()
        self.assertEqual(set(files), {'suite-disk.cfg', 'suite-root.img'})
        request = disk.HEADER.unpack(files['suite-disk.cfg'].read_bytes())
        self.assertEqual(request[1:3], (1, 4096))
        self.assertEqual(request[3], metal_root.root_record('prior', 4096, 'a' * 64))
        self.assertEqual(request[4], disk.record(facts))
        self.assertEqual((self.inputs / 'ext2.img').read_bytes()[:512], disk.record(facts))
        existing.m.write_json(existing.m.STATE / 'disk.json', facts)
        with patch.object(metal_root, 'stage_root') as root:
            reused, files = disk.stage(self.directory, self.inputs, 'next', 'c' * 40, self.site, self.build)
            root.assert_not_called()
        self.assertEqual(files, {})
        self.assertEqual(reused['installedBy'], 'first')
        self.assertEqual(reused['buildDigest'], facts['buildDigest'])
        # A changed userspace digest must reinstall even at the same commit.
        (self.inputs / 'ext2.img').write_bytes(bytes(1080) + b'\x53\xef' + bytes(3014))
        self.build['ext2']['sha256'] = 'd' * 64
        with patch.object(metal_root, 'stage_root') as root:
            changed, files = self.stage('changed')
            root.assert_called_once()
        self.assertNotEqual(changed['buildDigest'], facts['buildDigest'])
        self.assertIn('suite-root.img', files)

    def test_lease_handoff_keeps_competitors_out_and_preserves_the_live_unit(self):
        m = existing.m
        self.ready()
        m.claim_lease(dict(id='suite', unit='suite-unit'), wait=False)
        self.assertFalse(m.handoff_lease('wrong-owner', dict(id='readback', unit='suite-unit')))
        with self.assertRaises(m.Busy):
            m.claim_lease(dict(id='competitor'), wait=False)
        self.assertTrue(m.handoff_lease('suite', dict(id='readback', unit='suite-unit')))
        self.assertEqual(m.read_lease()['unit'], 'suite-unit')
        self.assertFalse(m.release_lease('suite'))
        with self.assertRaises(m.Busy):
            m.claim_lease(dict(id='competitor'), wait=False)
        m.quarantine_lease('readback', 'power state uncertain')
        self.assertFalse(m.handoff_lease('readback', dict(id='third', unit='suite-unit')))

    def test_cold_readback_refuses_an_intervening_boot_even_with_the_same_root(self):
        source_id = '20261018T000000Z-metal'
        source = existing.m.boot_dir(source_id)
        source.mkdir(parents=True)
        facts = dict(installedBy='prior', bytes=4096, sha256AfterRecord='a' * 64)
        existing.m.write_json(source / 'run.json', dict(disk='root', mode='suite'))
        existing.m.write_json(source / 'suite-disk.json', facts)
        (source / 'commit').write_text('b' * 40)
        (source / 'ended').touch()
        events = [dict(event='power-off', off=True, t=1), dict(event='decided', status=0),
                  dict(event='disk-root-record', matchesLastInstall=True)]
        (source / 'events.jsonl').write_text('\n'.join(json.dumps(e) for e in events))
        existing.m.write_json(existing.m.STATE / 'disk.json', dict(facts, lastBoot='another-boot'))
        with self.assertRaisesRegex(ValueError, 'replaced between'):
            disk.prepare_readback(self.directory, self.inputs, source_id, 'b' * 40, 'readback', self.site, self.inputs / 'stub.efi')


if __name__ == '__main__':
    unittest.main()
