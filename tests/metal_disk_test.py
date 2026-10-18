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

    def test_each_boot_reinstalls_even_unchanged_userspace_and_credentials(self):
        original = (self.inputs / 'ext2.img').read_bytes()
        previous = existing.m.read_json(existing.m.STATE / 'disk.json')
        for boot, commit in [('first', 'b' * 40), ('next', 'c' * 40), ('same-commit', 'c' * 40)]:
            (self.inputs / 'ext2.img').write_bytes(original)
            with patch.object(metal_root, 'stage_root') as root:
                facts, files = disk.stage(self.directory, self.inputs, boot, commit, self.site, self.build,
                                         mode='suite', suite='files-io')
                root.assert_called_once()
                self.assertEqual(root.call_args.args[1:3], ('suite', 'files-io'))
            self.assertEqual(set(files), {'suite-disk.cfg', 'suite-root.img'})
            request = disk.HEADER.unpack(files['suite-disk.cfg'].read_bytes())
            self.assertEqual(request[1:3], (1, 4096))
            self.assertEqual(request[3], disk.record(previous))
            self.assertEqual(request[4], disk.record(facts))
            self.assertEqual(facts['installedBy'], boot)
            existing.m.write_json(existing.m.STATE / 'disk.json', facts)
            previous = facts
        # Private credentials are also staged anew, even with unchanged ext2.
        (self.inputs / 'ext2.img').write_bytes(original)
        self.site['CONSOLE_PASSWORD'] += '_changed'
        with patch.object(metal_root, 'stage_root') as root:
            changed, files = self.stage('credentials')
            self.assertEqual(root.call_args.args[3]['CONSOLE_PASSWORD'], self.site['CONSOLE_PASSWORD'])
        self.assertNotEqual(changed['buildDigest'], previous['buildDigest'])
        self.assertIn('suite-root.img', files)

    def test_default_disk_boot_carries_current_boot_intent(self):
        fixture = existing.DriverTest()
        fixture.setUp()
        try:
            d = fixture.driver
            d.args.mode, d.args.disk, d.args.commit = 'default', 'root', 'b' * 40
            d.args.ramdisk = True
            inputs = d.stage / 'inputs'
            inputs.mkdir(parents=True)
            (inputs / 'ready').touch()
            existing.m.write_json(inputs / 'build.json', dict(commit=d.args.commit, profile='production', features=[]))
            with patch.object(existing.boot.metal_disk, 'stage', return_value=({}, {})) as stage, \
                 patch.object(existing.boot.metal_image, 'extract_esp', return_value={}), \
                 patch.object(existing.boot.metal_image, 'assemble', side_effect=RuntimeError('captured')) as assemble:
                with self.assertRaisesRegex(RuntimeError, 'captured'):
                    d.prepare_payload()
            self.assertEqual(assemble.call_args.args[1]['ramdisk'].read_bytes(), b'BREENIX-BOOT 1\ndefault\n')
            self.assertEqual(stage.call_args.kwargs['mode'], 'default')
        finally:
            fixture.tearDown()

    def test_pre_mount_fault_replaces_only_the_delivered_bootloader(self):
        fixture = existing.DriverTest()
        fixture.setUp()
        try:
            d = fixture.driver
            d.args.mode, d.args.disk, d.args.commit = 'default', 'root', 'b' * 40
            d.args.inject = 'disk-before-mount'
            d.args.ramdisk = True
            inputs = d.stage / 'inputs'
            inputs.mkdir(parents=True)
            (inputs / 'ready').touch()
            loader = inputs / 'loader.efi'
            loader.write_bytes(b'original EFI image')
            existing.m.write_json(inputs / 'build.json', dict(commit=d.args.commit, profile='production', features=[]))
            with patch.object(existing.boot.metal_disk, 'stage', return_value=({}, {})), \
                 patch.object(existing.boot.metal_image, 'extract_esp', return_value={'efi/boot/bootx64.efi': loader}), \
                 patch.object(existing.boot.metal_image, 'assemble', side_effect=RuntimeError('captured')):
                with self.assertRaisesRegex(RuntimeError, 'captured'):
                    d.prepare_payload()
            self.assertEqual(loader.read_bytes(), b'injected invalid EFI image\n')
        finally:
            fixture.tearDown()

    def test_install_record_survives_a_kernel_panic_before_mount(self):
        fixture = existing.DriverTest()
        fixture.setUp()
        try:
            d = fixture.driver
            d.args.disk = 'root'
            facts = dict(installedBy='fresh', bytes=4096, sha256AfterRecord='a' * 64)
            d.delivery['diskRoot'] = facts
            d.on_disk_line('metal-stub: suite fixture verified: 4096 bytes', 2)
            self.assertEqual(existing.m.read_json(existing.m.STATE / 'disk.json'), dict(facts, lastBoot=d.id))
            self.assertNotIn('disk-root', d.seen)
            # A short fixture must not replace the ownership record.
            d.seen.discard('disk-fixture')
            with self.assertRaises(existing.boot.Finish):
                d.on_disk_line('metal-stub: suite fixture verified: 2048 bytes', 3)
            self.assertEqual(existing.m.read_json(existing.m.STATE / 'disk.json'), dict(facts, lastBoot=d.id))
        finally:
            fixture.tearDown()

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
