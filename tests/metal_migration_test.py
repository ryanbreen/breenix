"""Host checks for the R240 migration boundaries."""
import hashlib
import io
import struct
from unittest.mock import patch
import unittest

import metal_tools_test as existing
import metal_persist

m = existing.m


class MetalMigrationTest(existing.StateTest):
    def test_legacy_source_and_rescue_are_read_only(self):
        layout = metal_persist.add_breenfs_layout(metal_persist.new_layout())
        parts = layout['partitions']
        for phase, expected in (
            ('migrate', {'test': ('/', 'ro'), 'system': ('/migrate/ext2-system', 'ro'),
                         'home': ('/migrate/ext2-home', 'ro'), 'breenfs-system': ('/migrate/system', 'rw'),
                         'breenfs-home': ('/migrate/home', 'rw')}),
            ('rescue', {'system': ('/', 'ro'), 'home': ('/home', 'ro')}),
            ('cutover', {'breenfs-system': ('/', 'rw'), 'breenfs-home': ('/home', 'rw')}),
        ):
            with self.subTest(phase=phase):
                intent = metal_persist.migration_intent(layout, phase, 'example')
                lines = intent.decode().splitlines()
                self.assertIn('userland example', lines)
                self.assertEqual({line for line in lines if line.startswith('mount ')},
                                 {f"mount {parts[role]['guid']} {path} {mode}"
                                  for role, (path, mode) in expected.items()})

    def test_backup_history_requires_the_test_answer_and_complete_index(self):
        import metal_migration

        base = metal_migration.COPY_OFF
        child = '20261121T100000Z-metal'
        rows = [dict(seq=40, id=base), dict(seq=41, id=child)]
        boots = m.STATE / 'boots'
        for ident in (base, child):
            (boots / ident).mkdir(parents=True)
            (boots / ident / 'ended').touch()
        m.write_json(boots / child / 'run.json', dict(id=child, payload='image', mode='suite',
                                                      disk='root', kind='fault-test'))
        events = [dict(event='power-on'), dict(event='manager-answer', answer=f'test {child}'),
                  dict(event='closed')]
        self.assertEqual(metal_migration.prove_history(rows, boots, lambda _: events), [child])
        for wrong in ('persistent unexpected current', 'test wrong'):
            with self.assertRaises(ValueError):
                metal_migration.prove_history(rows, boots, lambda _: [
                    events[0], dict(event='manager-answer', answer=wrong), events[2]])
        rows[1]['seq'] += 1
        with self.assertRaises(ValueError):
            metal_migration.prove_history(rows, boots, lambda _: events)

    def test_host_comparison_requires_both_guest_digests(self):
        import metal_migration

        row = b"2f\tdir\t0755\t0\t0\t-\t0\t0.000000000\t-\n"
        generation = row.replace(b"0755", b"0700")
        body = metal_migration.join_manifests(row, generation, row, 'v1')
        self.assertEqual([line.split(b'\t')[0] for line in body.splitlines()],
                         [b'2f', b'2f67656e2d7631', b'2f686f6d65'])
        digest = hashlib.sha256(body).hexdigest()
        metal_migration.compare_guest_manifests(body, body, dict(source=digest, destination=digest))
        with self.assertRaises(ValueError):
            metal_migration.compare_guest_manifests(body, body, dict(source=digest, destination='0' * 64))
        with self.assertRaises(ValueError):
            metal_migration.compare_guest_manifests(body, body.replace(b'0700', b'0777'),
                                                    dict(source=digest, destination=digest))

    def test_cutover_requires_cold_proof_and_idle_manager_answers_test(self):
        import metal_migration
        from metal_persistent_test import http

        boot = self._testMethodName
        commit = 'a' * 40
        layout = metal_persist.add_breenfs_layout(metal_persist.new_layout())
        before = dict(commit=commit, userland='v1', add='20261121T101000Z-metal',
                      boots=dict(add='a', migrate='b'), layout=layout)
        metal_migration.save(before)
        metal_persist.update_disk(layout=layout)
        self.assertEqual(http.manager_answer(), 'test ' + before['add'])
        with self.assertRaises(ValueError):
            metal_migration.admit('cutover', boot, commit, commit)
        with patch.object(metal_migration, 'backup') as backup:
            before['boots']['readback'] = 'c'
            metal_migration.save(before)
            self.assertEqual(metal_migration.admit('cutover', boot, commit, commit), before)
            backup.assert_called_once_with(layout, allowed={'a': 'add', 'b': 'migrate', 'c': 'readback'},
                                           active=boot)

    def test_sparse_stream_preserves_offsets_and_rejects_truncation(self):
        import metal_migration

        block = 4096
        selected = [16, 17, 18, 19, 32, 60, 61, 62, 63]
        bitmap = sum(1 << n for n in selected).to_bytes(8, 'little')
        data = b''.join(bytes([n]) * block for n in selected)
        body = struct.pack('<Q', len(selected)) + bitmap + data
        out = io.BytesIO()
        digest = metal_migration.unpack_sparse(io.BytesIO(body), out, 64 * block, len(body))
        self.assertEqual(digest, hashlib.sha256(body).hexdigest())
        out.seek(32 * block)
        self.assertEqual(out.read(2 * block), bytes([32]) * block + bytes(block))
        out.seek(60 * block)
        self.assertEqual(out.read(), data[-4 * block:])
        with self.assertRaises(ValueError):
            metal_migration.unpack_sparse(io.BytesIO(body[:-1]), io.BytesIO(), 64 * block, len(body))


if __name__ == '__main__':
    unittest.main()
