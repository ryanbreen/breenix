"""Host evidence for the normal upgrade path after the recorded migration."""
import unittest
from unittest.mock import patch

import metal_tools_test as existing
import metal_migration as migration
import metal_persist as persist
import upgrade_cases

m = existing.m
RETURN = '20261121T172024Z-metal'
CYCLE_COMMIT = '2aa932f2f61542f111c23be70dd824f7ba7fe411'
STAGED_COMMIT = '94109fc7b3d95137cb91289f6a9f1321409cbcf6'
RETURN_COMMIT = '250fb833de97abddbf4672dbfca1980cb370cfe3'
COMMIT = 'a' * 40


class BreenfsUpgradeTest(existing.StateTest):
    def setUp(self):
        super().setUp()
        self.layout = persist.add_breenfs_layout(persist.new_layout())
        self.original = '20261121T080000Z-metal'
        self.slots = dict(
            current=dict(commit=STAGED_COMMIT, installedBy=self.original, migration='cutover'),
            previous=dict(commit=STAGED_COMMIT, installedBy=self.original, migration='rescue'),
        )
        persist.update_disk(layout=self.layout, slots=self.slots)
        self.cycle = dict(add='20261121T152420Z-metal', commit=CYCLE_COMMIT, layout=self.layout,
                          userland=self.original,
                          boots=dict(zip(migration.PHASES, [
                              '20261121T152420Z-metal', '20261121T154016Z-metal',
                              '20261121T154804Z-metal', '20261121T164442Z-metal',
                              '20261121T171825Z-metal', RETURN,
                          ], strict=True)))
        migration.save(self.cycle)
        returned = m.boot_dir(RETURN)
        returned.mkdir(parents=True)
        m.write_json(returned / 'run.json', dict(id=RETURN, migrationPhase='return',
                    payload='persistent', entry='current', commit=RETURN_COMMIT,
                    toolsCommit=RETURN_COMMIT, bootedCommit=STAGED_COMMIT))
        m.write_json(returned / 'filesystem-persistent-root.json', dict(
            milestone='filesystem/persistent-root', boot=RETURN, boots=self.cycle['boots'],
            commit=CYCLE_COMMIT, sourceCommit=RETURN_COMMIT, toolsCommit=RETURN_COMMIT, bootedCommit=STAGED_COMMIT,
            cleanup=True, consoleGaps=[], cases=[
                dict(id='persistent-root/' + name, result='PASS', ms=0) for name in (
                    'migrate-manifests-equal', 'host-manifests-equal', 'cutover-verified', 'rescue-and-return')]))
        m.write_json(m.STATE / 'machine.json', dict(sample=dict(power='Off')))
        m.event(returned, 'manager-answer', answer=f'persistent {RETURN} current')
        m.event(returned, 'power-off', off=True)
        m.close_record(returned, 0, 'driver')

    def test_closes_only_the_recorded_successful_return_and_keeps_test_routing(self):
        with self.assertRaisesRegex(ValueError, 'closed'):
            migration.require_closed(self.layout)
        with self.assertRaises(ValueError):
            migration.close(self.cycle['boots']['rescue'])
        exit_status = m.boot_dir(RETURN) / 'exit-status'
        exit_status.write_text('1\n')
        with self.assertRaises(ValueError):
            migration.close(RETURN)
        exit_status.write_text('0\n')
        run = m.boot_dir(RETURN) / 'run.json'
        recorded = m.read_json(run)
        m.write_json(run, dict(recorded, bootedCommit=RETURN_COMMIT))
        with self.assertRaises(ValueError):
            migration.close(RETURN)
        m.write_json(run, recorded)
        with patch.object(m, 'read_lease', return_value={'id': RETURN}):
            with self.assertRaises(ValueError):
                migration.close(RETURN)
        migration.close(RETURN)
        self.assertEqual(migration.require_closed(self.layout)['closed'], RETURN)
        self.assertEqual(existing.load_script('metal-http').manager_answer(), 'test ' + self.cycle['add'])
        self.assertEqual(migration.close(RETURN)['closed'], RETURN)

    def test_normal_upgrade_stages_only_breenfs_persistent_writes(self):
        migration.close(RETURN)
        current = '20261121T180000Z-metal'
        stage = m.STATE / 'upgrade'
        stage.mkdir()
        source = stage / 'source'
        source.write_bytes(b'payload')
        parts = self.layout['partitions']
        with patch.object(persist.metal_image, 'assemble') as assemble:
            persist.slot_image(stage / 'slot', dict.fromkeys(persist.metal_image.ESP_FILES, source),
                               self.layout, stage, current)
        self.assertEqual(assemble.call_args.args[1]['ramdisk'].read_bytes(),
                         persist.migration_intent(self.layout, 'cutover', current))
        intent = persist.breenfs_upgrade_intent(self.layout, current)
        self.assertEqual(intent.decode().splitlines(), [
            'BREENIX-BOOT 3', 'target persistent-install',
            f"mount {parts['test']['guid']} / ro",
            f"mount {parts['breenfs-system']['guid']} /migrate/system rw",
            f"mount {parts['breenfs-home']['guid']} /migrate/home rw",
            'userland ' + current,
        ])
        config, _ = persist.upgrade_request(stage, self.layout, {}, source, source,
                                            {'BOOT_URL': 'http://example.invalid/boot'}, breenfs=True)
        self.assertIn('\nbreenfs-upgrade ', config.read_text())
        self.assertNotIn('\nrescue-file ', config.read_text())

    def test_two_upgrades_require_distinct_chained_breenfs_verifications(self):
        mounts = '\n'.join(f'disk {path} breenfs {flags} 0 0' for path, flags in [
            ('/', 'ro'), ('/home', 'rw,nosuid,nodev'),
            *((path, 'rw') for path in ('/etc', '/var', '/opt', '/srv', '/tmp', '/run', '/root')),
        ])

        def receipt(install, child, previous, fingerprint='pin'):
            return dict(install=install, candidateGeneration=install, previousGeneration=previous,
                        toolsCommit=COMMIT, outcome='verified',
                        verification=dict(boot=child, exit=0, passed=True, authenticated=True,
                                          strictHostKey=True, servicesStarted=True,
                                          generation=install, mounts=mounts, homeReadable=True,
                                          fingerprint=fingerprint, powerOn=1.0, completedAt=2.0))

        before = dict(self.cycle, closed=RETURN)
        one = receipt('20261121T180000Z-metal', '20261121T180100Z-metal', self.original)
        two = receipt('20261121T181000Z-metal', '20261121T181100Z-metal', one['install'])
        self.assertEqual(upgrade_cases.breenfs_pair(before, COMMIT, one, two)['cases'][0]['id'], 'ext2-retired/two-upgrades')
        for bad in (one, dict(two, previousGeneration=self.original), dict(two, outcome='rolled-back'),
                    dict(two, verification=dict(two['verification'], boot=one['verification']['boot'])),
                    dict(two, verification=dict(two['verification'], mounts=mounts.replace('breenfs', 'ext2'))),
                    dict(two, verification=dict(two['verification'], fingerprint='changed'))):
            with self.subTest(bad=bad), self.assertRaises(ValueError):
                upgrade_cases.breenfs_pair(before, COMMIT, one, bad)

    def test_records_the_two_install_and_s21_boots_after_the_return(self):
        migration.close(RETURN)
        previous = self.original
        for install, child in [('20261121T180000Z-metal', '20261121T180100Z-metal'),
                               ('20261121T181000Z-metal', '20261121T181100Z-metal')]:
            for ident, mode in [(install, 'persistent-install'), (child, 'persistent')]:
                directory = m.boot_dir(ident)
                directory.mkdir()
                run = dict(id=ident, mode=mode, commit=COMMIT, toolsCommit=COMMIT, breenfsUpgrade=RETURN)
                if ident == child:
                    run.update(upgradeParent=install, bootedCommit=COMMIT, entry='current')
                    m.event(directory, 'power-on', observed_at=1)
                    m.event(directory, 'manager-answer', answer=f'persistent {child} current commit')
                m.write_json(directory / 'run.json', run)
                m.event(directory, 'power-off', off=True)
                m.close_record(directory, 0, 'driver')
            m.event(m.boot_dir(install), 'persistent-verification-next', verification=child)
            mounts = '\n'.join(f'disk {path} breenfs {flags} 0 0' for path, flags in [
                ('/', 'ro'), ('/home', 'rw,nosuid,nodev'),
                *((path, 'rw') for path in ('/etc', '/var', '/opt', '/srv', '/tmp', '/run', '/root')),
            ])
            receipt = dict(boot=child, passed=True, authenticated=True, strictHostKey=True,
                           servicesStarted=True, homeReadable=True, mounts=mounts,
                           fingerprint='pin', generation=install, powerOn=1, completedAt=2)
            m.write_json(m.boot_dir(child) / 'ssh-verification.json', receipt)
            m.write_json(m.boot_dir(install) / 'upgrade-result.json', dict(
                install=install, candidateGeneration=install, previousGeneration=previous,
                outcome='verified', toolsCommit=COMMIT, verification=dict(receipt, exit=0)))
            previous = install

        first = '20261121T180000Z-metal'
        proof = migration.record_upgrades(first, install, COMMIT)
        self.assertEqual(m.read_json(m.boot_dir(child) / 'filesystem-two-upgrades.json'), proof)
        (m.boot_dir(child) / 'ssh-verification.json').unlink()
        with self.assertRaises(ValueError):
            migration.record_upgrades(first, install, COMMIT)


if __name__ == '__main__':
    unittest.main()
