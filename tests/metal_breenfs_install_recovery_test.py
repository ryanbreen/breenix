"""Replay the stopped E24 install and exercise its host-side custody."""
import json
import os
from pathlib import Path
import shutil
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch

import metal_breenfs_upgrade_test as upgrade
import metal_score

existing = upgrade.existing
boot = existing.boot
m = upgrade.m
migration = upgrade.migration
persist = upgrade.persist
STOPPED = '20261121T201545Z-metal'
STOPPED_COMMIT = '1a71e50d60315a94c4006e48e32740cfc762539c'
RETRIES = [('20261121T210000Z-metal', '20261121T210100Z-metal'),
           ('20261121T211000Z-metal', '20261121T211100Z-metal')]
FIXTURE = Path(__file__).with_name('fixtures') / 'metal-breenfs-install-20261121T201545Z.serial'
FENCE = '[block] write fence: AHCI port 0 LBA 62916608-197134335, 197134336-734005247'
CATALOG = 'tools/breenix-runs/Sources/BreenixRuns/Resources/boot-stages-x86_64.json'


class BreenfsInstallReplayTest(existing.StateTest):
    def replay(self, serial):
        args = SimpleNamespace(id=STOPPED, mode='persistent-install', payload='image', disk='root',
                               inject=None, breenfs_upgrade=upgrade.RETURN)
        with patch.object(m, 'load_site', return_value={}):
            driver = boot.Driver(args)
        driver.t0 = 100
        driver.phase = driver.event = driver.on_manager_line = driver.on_persist_line = Mock()
        # Replay the completion matcher with the separately checked delivery receipt.
        driver.seen.add('disk-root')
        driver.on_disk_line = Mock()
        driver.persist = {'layout': persist.add_breenfs_layout(persist.new_layout())}
        for relative in ('docs/metal/boot-path.json', CATALOG):
            destination = driver.dir / 'definition' / relative
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(existing.TOOLS.parents[1] / relative, destination)
        m.write_json(driver.dir / 'persist.json', driver.persist)
        m.write_json(driver.dir / 'run.json', dict(mode=args.mode, disk=args.disk,
                     breenfsUpgrade=args.breenfs_upgrade))
        (driver.dir / 'serial.txt').write_text(serial)
        (driver.dir / 'events.jsonl').write_text(''.join(json.dumps(dict(event=name)) + '\n'
            for name in ('console-ready', 'power-on', 'firmware-text', 'post-code', 'image-complete', 'handoff')))
        driver.load_definition()
        status = None
        try:
            for line in serial.splitlines():
                driver.on_line(line, observed_at=200)
        except boot.Finish as result:
            status = result.status
        verdict = metal_score.score(driver.dir)['milestones']['disk-filesystem']
        return status, verdict, driver.required_stages

    def test_retained_serial_reaches_install_completion(self):
        # The fixture preserves the stage lines in order, with site values replaced.
        # Set BREENIX_E24_INSTALL_CAPTURE to replay the complete private capture.
        serial = Path(os.environ.get('BREENIX_E24_INSTALL_CAPTURE', FIXTURE)).read_text()
        self.assertIn(FENCE, serial)
        status, verdict, pending = self.replay(serial)
        self.assertEqual(status, m.EXIT_DONE, pending)
        self.assertTrue(verdict['passed'], verdict)

    def test_fenced_install_cannot_finish_without_install_complete(self):
        serial = FIXTURE.read_text()
        before_completion = serial[:serial.index(boot.INSTALL_DONE)]
        status, verdict, pending = self.replay(before_completion)
        self.assertNotEqual(status, m.EXIT_DONE, pending)
        self.assertFalse(verdict['passed'], verdict)

    def test_other_fences_cannot_finish_the_install(self):
        serial = FIXTURE.read_text()
        for changed in (FENCE + ', 1-2', FENCE.replace('734005247', '734005248'),
                        FENCE + '\n' + FENCE.replace('734005247', '734005248')):
            with self.subTest(fence=changed):
                status, verdict, _ = self.replay(serial.replace(FENCE, changed))
                self.assertNotEqual(status, m.EXIT_DONE)
                self.assertFalse(verdict['passed'], verdict)


class StoppedUpgradeTest(upgrade.BreenfsUpgradeFixture):
    def attempt(self, ident=STOPPED, *, answer=None, selection=None, fence=FENCE, off=True,
                status=143, ended_by='driver'):
        directory = m.boot_dir(ident)
        directory.mkdir(exist_ok=True)
        slot = dict(installedBy=ident, commit=STOPPED_COMMIT, size=512, sha256='b' * 64)
        m.write_json(directory / 'run.json', dict(id=ident, mode='persistent-install', disk='root',
                     payload='image', kind='boot', commit=STOPPED_COMMIT, toolsCommit=STOPPED_COMMIT,
                     breenfsUpgrade=upgrade.RETURN))
        m.write_json(directory / 'persist.json', dict(action='upgrade', layout=self.layout, slot=slot))
        (directory / 'serial.txt').write_text(FIXTURE.read_text().replace(FENCE, fence))
        (directory / 'events.jsonl').write_text('')
        m.event(directory, 'power-on')
        m.event(directory, 'manager-answer', answer=answer or f'test {ident}')
        if selection:
            m.event(directory, 'slot-boot', entry=selection)
        m.event(directory, 'persist-written', action='upgrade', commit=STOPPED_COMMIT)
        m.event(directory, 'persistent-install-complete')
        m.event(directory, 'power-off', off=off)
        if not m.close_record(directory, status, ended_by):
            m.event(directory, 'closed', status=status, endedBy=ended_by)
            (directory / 'exit-status').write_text(f'{status}\n')
            (directory / 'ended-by').write_text(ended_by + '\n')
        return slot

    def custody(self, ident=STOPPED, *, line=1040):
        record = m.STATE / 'migration-carry-forward.jsonl'
        m.write_text(record, '')
        m.append_jsonl(record, dict(ledgerLine=line, boot=ident))

    def test_only_the_named_stopped_install_is_admitted_to_history(self):
        migration.close(upgrade.RETURN)
        self.attempt()
        first, second, _ = self.completed_upgrades(RETRIES)
        expected = [dict(install=install, verification=child) for install, child in RETRIES]

        def upgrades():
            return migration.upgrade_proof(first, second, upgrade.COMMIT)['upgrades']

        def history():
            return migration.reclaim_history(upgrade.RETURN, '20261121T220000Z-metal', expected)

        def refused():
            for check in (upgrades, history):
                with self.assertRaises(ValueError):
                    check()

        refused()
        self.custody(line=1021)
        refused()
        self.custody()
        with self.subTest(check='two verified upgrades'):
            self.assertEqual([(row['install'], row['verification']) for row in upgrades()], RETRIES)
        with self.subTest(check='reclaim history'):
            self.assertIn(STOPPED, history())
        with self.assertRaises(ValueError):
            migration.upgrade_proof(STOPPED, second, upgrade.COMMIT)
        for changed in (dict(answer='test 20261121T201546Z-metal'), dict(selection='current'),
                        dict(fence=FENCE + ', 1-2'), dict(off=False), dict(status=124), dict(ended_by='reaper')):
            with self.subTest(changed=changed):
                self.attempt(**changed)
                refused()
        self.attempt()
        other = '20261121T205000Z-metal'
        self.attempt(other)
        m.append_jsonl(m.STATE / 'migration-carry-forward.jsonl', dict(ledgerLine=1040, boot=other))
        refused()

    def test_upgrade_replaces_only_the_admitted_next_and_keeps_current_and_rescue(self):
        migration.close(upgrade.RETURN)
        old_slots = dict(self.slots, rescue=self.slots['previous'], next=self.attempt())
        persist.update_disk(slots=old_slots)
        args = SimpleNamespace(id=RETRIES[0][0], mode='persistent-install', payload='image',
                               inject=None, commit=upgrade.COMMIT)
        with patch.object(m, 'load_site', return_value={'BOOT_URL': 'http://example.invalid/boot'}):
            driver = boot.Driver(args)
        driver.dir.mkdir()
        driver.phase = driver.event = Mock()
        inputs = driver.dir / 'inputs'
        inputs.mkdir()
        manager = inputs / 'manager.efi'
        manager.write_bytes(b'manager')
        build = dict(manager=dict(sha256=boot.metal_image.sha256(manager)))
        with patch.object(boot.metal_image, 'extract_esp', return_value={}), \
             patch.object(boot.metal_disk, 'stage', return_value=({}, {})), \
             patch.object(persist, 'slot_image', side_effect=lambda path, *_: path.write_bytes(b'candidate')), \
             patch.object(persist, 'store_definition'):
            with self.assertRaisesRegex(boot.Finish, 'NEXT'):
                driver.prepare_install(inputs, build)
            self.custody()
            for other in (dict(old_slots['next'], installedBy='20261121T205000Z-metal'),
                          dict(old_slots['next'], sha256='c' * 64)):
                persist.update_disk(slots=dict(old_slots, next=other))
                with self.assertRaisesRegex(boot.Finish, 'NEXT'):
                    driver.prepare_install(inputs, build)
            persist.update_disk(slots=old_slots)
            args.slot_action = 'commit'
            with self.assertRaisesRegex(boot.Finish, 'NEXT'):
                driver.prepare_persistent()
            args.slot_action = None
            driver.prepare_install(inputs, build)
            self.assertEqual(persist.read_disk()['slots'], old_slots)
            driver.on_persist_line('metal-stub: BreenFS upgrade: ext2 rescue pair preserved', 1)
            driver.on_persist_line(boot.PERSIST_DONE, 2)
            self.assertEqual(persist.read_disk()['slots'], dict(old_slots, next=driver.persist['slot']))
            self.assertEqual(driver.persist['slot']['installedBy'], args.id)


if __name__ == '__main__':
    unittest.main()
