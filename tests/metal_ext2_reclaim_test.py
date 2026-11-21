"""Exercise ext2 retirement against the same upgrade receipts used at acceptance."""

import shutil
import unittest
import uuid
from unittest.mock import patch

import metal_breenfs_upgrade_test as upgrade

m = upgrade.m
persist = upgrade.persist
migration = upgrade.migration
FIRST = '20261121T180000Z-metal'
SECOND = '20261121T181000Z-metal'
VERIFIED = '20261121T181100Z-metal'
RECLAIM = '20261121T190000Z-metal'


class MetalExt2ReclaimTest(upgrade.BreenfsUpgradeFixture):
    def test_upgrade_config_offers_rescue_only_for_the_ext2_pair(self):
        stage = m.STATE / 'upgrade'
        stage.mkdir()
        source = stage / 'payload'
        source.write_bytes(b'slot')
        guids = [uuid.UUID(self.layout['partitions'][role]['guid']).bytes_le.hex()
                 for role in ('system', 'home')]
        site = {'BOOT_URL': 'http://example.invalid/boot'}
        config, _ = persist.upgrade_request(stage, self.layout, {}, source, source, site, breenfs=True)
        self.assertIn('manager-config rescue-partitions ' + ' '.join(guids), config.read_text().splitlines())
        config, _ = persist.upgrade_request(stage, persist.reclaimed_layout(self.layout), {},
                                            source, source, site, breenfs=True)
        self.assertNotIn('manager-config rescue-partitions ', config.read_text())

    def failed_attempt(self, ident, payload, mode, *, powered=False):
        boot = m.boot_dir(ident)
        boot.mkdir()
        m.write_json(boot / 'run.json', dict(id=ident, payload=payload, mode=mode, kind='fault-test'))
        if powered:
            m.event(boot, 'power-on')
            m.event(boot, 'manager-answer', answer=f'test {ident}')
            m.event(boot, 'power-off', off=True)
        m.close_record(boot, m.EXIT_SHORT if powered else m.EXIT_CANCELLED, 'driver')
        return boot

    def test_reclaim_history_allows_a_refusal_before_power_on(self):
        refused = '20261121T175000Z-metal'
        self.failed_attempt(refused, 'reclaim-ext2', 'reclaim-ext2')
        self.completed_upgrades()
        proof = migration.record_upgrades(FIRST, SECOND, upgrade.COMMIT)
        self.assertIn(refused, migration.reclaim_history(upgrade.RETURN, RECLAIM, proof['upgrades']))

    def test_reclaim_history_allows_a_powered_refusal_only_on_the_test_route(self):
        self.completed_upgrades()
        proof = migration.record_upgrades(FIRST, SECOND, upgrade.COMMIT)
        refused = '20261121T183000Z-metal'
        boot = self.failed_attempt(refused, 'reclaim-ext2', 'reclaim-ext2', powered=True)
        self.assertIn(refused, migration.reclaim_history(upgrade.RETURN, RECLAIM, proof['upgrades']))
        m.event(boot, 'slot-boot', entry='previous')
        with self.assertRaisesRegex(ValueError, 'rescue'):
            migration.reclaim_history(upgrade.RETURN, RECLAIM, proof['upgrades'])

    def test_refused_install_does_not_count_as_one_of_the_two_upgrades(self):
        refused = '20261121T175000Z-metal'
        boot = self.failed_attempt(refused, 'image', 'persistent-install')
        self.completed_upgrades()
        proof = migration.record_upgrades(FIRST, SECOND, upgrade.COMMIT)
        self.assertIn(refused, migration.reclaim_history(upgrade.RETURN, RECLAIM, proof['upgrades']))
        m.event(boot, 'power-on-error')
        with self.assertRaisesRegex(ValueError, 'paired install'):
            migration.record_upgrades(FIRST, SECOND, upgrade.COMMIT)

    def test_reclaim_requires_lease_verified_upgrades_and_unused_rescue(self):
        with self.assertRaisesRegex(ValueError, 'six-entry'):
            persist.reclaim_request(m.STATE, persist.new_layout(), 'first', 'second')
        # Exercise the real l1 verifier with its install and verification records.
        self.assertEqual(self.completed_upgrades(), (FIRST, SECOND, VERIFIED))
        migration.record_upgrades(FIRST, SECOND, upgrade.COMMIT)
        result = m.boot_dir(SECOND) / 'upgrade-result.json'
        receipt = m.read_json(result)['verification']
        ssh = m.boot_dir(VERIFIED) / 'ssh-verification.json'
        m.write_json(ssh, {key: value for key, value in receipt.items() if key != 'exit'})
        persist.update_disk(slots=dict(
            current=dict(commit=upgrade.COMMIT, installedBy=SECOND),
            previous=dict(commit=upgrade.COMMIT, installedBy=FIRST),
            rescue=self.slots['previous'],
        ))
        with self.assertRaisesRegex(ValueError, 'lease'):
            migration.admit_reclaim(RECLAIM, FIRST, SECOND, upgrade.COMMIT, upgrade.COMMIT)

        lease = {'id': RECLAIM, 'state': 'claimed', 'payload': 'reclaim-ext2'}
        with patch.object(m, 'read_lease', return_value=lease):
            ssh.unlink()
            with self.assertRaisesRegex(ValueError, 'verification'):
                migration.admit_reclaim(RECLAIM, FIRST, SECOND, upgrade.COMMIT, upgrade.COMMIT)
            m.write_json(ssh, {key: value for key, value in receipt.items() if key != 'exit'})

            rescue = m.boot_dir('20261121T182000Z-metal')
            rescue.mkdir()
            m.write_json(rescue / 'run.json', dict(id=rescue.name, payload='persistent', entry='previous'))
            m.event(rescue, 'manager-answer', answer=f'persistent {rescue.name} previous')
            m.event(rescue, 'power-off', off=True)
            index = m.STATE / 'finished.jsonl'
            original = index.read_bytes()
            m.close_record(rescue, 0, 'driver')
            with self.assertRaisesRegex(ValueError, 'rescue'):
                migration.admit_reclaim(RECLAIM, FIRST, SECOND, upgrade.COMMIT, upgrade.COMMIT)
            shutil.rmtree(rescue)
            index.write_bytes(original)

            proof = migration.admit_reclaim(RECLAIM, FIRST, SECOND, upgrade.COMMIT, upgrade.COMMIT)
            old = persist.read_disk()
            config, request = persist.reclaim_request(m.STATE, self.layout, FIRST, SECOND)
            lines = config.read_text().splitlines()
            self.assertIn('action reclaim-ext2', lines)
            self.assertIn(f'keep-generations {SECOND} {FIRST}', lines)
            self.assertEqual(sum(line.startswith('partition ') for line in lines), 6)
            self.assertEqual(request.data.stat().st_size, 0)
            self.assertEqual(persist.read_disk(), old)

            boot = m.boot_dir(RECLAIM)
            boot.mkdir()
            m.write_json(boot / 'run.json', dict(id=RECLAIM, payload='reclaim-ext2',
                         commit=upgrade.COMMIT, toolsCommit=upgrade.COMMIT))
            m.event(boot, 'manager-answer', answer=f'test {RECLAIM}')
            lease['state'] = 'powered'
            with self.assertRaisesRegex(ValueError, 'readback'):
                migration.record_reclaimed(RECLAIM, proof, old)
            self.assertEqual(persist.read_disk(), old)
            m.event(boot, 'ext2-gpt-readback')
            m.event(boot, 'power-off', off=True)
            reclaimed = migration.record_reclaimed(RECLAIM, proof, old)
            new = persist.read_disk()
            self.assertEqual(set(new['slots']), {'current', 'previous'})
            self.assertEqual(new['slots']['previous'], old['slots']['previous'])
            self.assertEqual(new['layout']['partitions'], {
                role: part for role, part in old['layout']['partitions'].items()
                if part['number'] in (1, 2, 5, 6)
            })
            self.assertEqual(persist.layout(), new['layout'])
            self.assertEqual(migration.require_closed(new['layout'])['closed'], upgrade.RETURN)
            self.assertEqual(reclaimed['cases'][0]['id'], 'ext2-retired/reclaimed')
            self.assertEqual(m.read_json(m.boot_dir(RECLAIM) / 'filesystem-reclaimed.json'), reclaimed)


if __name__ == '__main__':
    unittest.main()
