"""Host checks for the R240 migration boundaries."""
import hashlib
import io
from pathlib import Path
import struct
import subprocess
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

    def test_backup_history_refuses_the_raw_write_fence_probe(self):
        import metal_migration

        child = '20261121T100000Z-metal'
        rows = [dict(seq=40, id=metal_migration.COPY_OFF), dict(seq=41, id=child)]
        boots = m.STATE / 'boots'
        (boots / child).mkdir(parents=True)
        (boots / child / 'ended').touch()
        m.write_json(boots / child / 'run.json', dict(id=child, kind='fault-test', payload='image',
                                                      mode='default', disk='root', inject='fence-out-of-partition'))
        events = [dict(event='manager-answer', answer=f'test {child}'), dict(event='closed')]
        with self.assertRaisesRegex(ValueError, 'could have reached the frozen ext2 partitions'):
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

    def test_add_can_be_retried_with_the_original_partition_ids(self):
        import metal_migration

        commit = 'a' * 40
        layout = metal_persist.new_layout()
        metal_persist.update_disk(layout=layout, slots={'current': {'installedBy': 'old'}})
        with patch.object(metal_migration, 'backup'):
            first = metal_migration.admit('add', '20261121T100000Z-metal', commit, commit)
            retry = metal_migration.admit('add', '20261121T101000Z-metal', commit, commit)
            metal_persist.update_disk(layout=first['layout'])
            retry = metal_migration.admit('add', '20261121T102000Z-metal', commit, commit)
            metal_persist.update_disk(layout=metal_persist.add_breenfs_layout(layout))
            with self.assertRaises(ValueError):
                metal_migration.admit('add', '20261121T103000Z-metal', commit, commit)
        self.assertEqual(retry['layout'], first['layout'])
        self.assertEqual(metal_migration.state()['add'], '20261121T102000Z-metal')

    def test_failed_readback_with_a_safe_route_does_not_block_retry(self):
        import metal_migration

        child = '20261121T100000Z-metal'
        rows = [dict(seq=40, id=metal_migration.COPY_OFF), dict(seq=41, id=child)]
        boots = m.STATE / 'boots'
        directory = boots / child
        directory.mkdir(parents=True)
        (directory / 'ended').touch()
        (directory / 'exit-status').write_text('1\n')
        commit = 'a' * 40
        m.write_json(directory / 'run.json',
                     dict(id=child, kind='fault-test', payload='persistent-export', disk=None, mode='default',
                          migrationPhase='readback', commit=commit, toolsCommit=commit))
        closed = dict(event='closed', status=1, endedBy='driver')
        self.assertEqual(metal_migration.prove_history(
            rows, boots, lambda _: [dict(event='migration-not-powered'), closed]), [child])
        with self.assertRaises(ValueError):
            metal_migration.prove_history(
                rows, boots, lambda _: [dict(event='baseline', power='On'),
                                        dict(event='migration-not-powered'), closed])

        admitted = dict(event='migration-admitted', phase='readback')
        power_on = dict(event='power-on')
        answer = dict(event='manager-answer', answer=f'test {child}')
        off = dict(event='power-off', off=True)
        self.assertEqual(metal_migration.prove_history(
            rows, boots, lambda _: [admitted, power_on, answer, off, closed]), [child])
        for log in ([power_on, answer, off, closed], [admitted, power_on, answer, closed],
                    [admitted, power_on, dict(event='manager-answer', answer=f'persistent {child} current'),
                     off, closed]):
            with self.assertRaises(ValueError):
                metal_migration.prove_history(rows, boots, lambda _: log)

    def test_shared_manifest_labels_only_retained_hard_links(self):
        import metal_migration

        empty_digest = hashlib.sha256(b'').hexdigest().encode()

        def row(path, group=b'-', kind=b'reg'):
            digest = b'-' if kind == b'dir' else empty_digest
            fields = (path.hex().encode(), kind, b'0755', b'0', b'0', group,
                      b'0', b'0.000000000', digest)
            return b'\t'.join(fields) + b'\n'

        dirs = {p: row(p, kind=b'dir') for p in (b'/', b'/.breenix-shared', b'/root')}
        for paths, retained, anchor in (
            ((b'/root/a', b'/unretained/a'), (b'/root/a',), b'-'),
            ((b'/.breenix-generations/v1/bin/b', b'/.breenix-shared/b', b'/root/b'),
             (b'/.breenix-shared/b', b'/root/b'), b'/.breenix-shared/b'.hex().encode()),
        ):
            with self.subTest(paths=paths):
                group = paths[0].hex().encode()
                source = dirs | {p: row(p, group) for p in paths}
                expected = dirs | {p: row(p, anchor) for p in retained}
                self.assertEqual(metal_migration.shared_manifest(b''.join(source[p] for p in sorted(source))),
                                 b''.join(expected[p] for p in sorted(expected)))

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


class CarryForwardAdmissionTest(existing.StateTest):
    allowed_files = ['scripts/metal/server-suite', 'scripts/metal/metal_migration.py',
                     'tests/remote_upgrade_test.py', 'tests/metal_migration_test.py']
    slot_repair_files = allowed_files + ['scripts/metal/metal-boot', 'tests/metal_persistent_test.py']

    def setUp(self):
        super().setUp()
        import metal_migration

        self.migration = metal_migration
        self.worktree = Path(self.temporary.name) / 'source'
        self.worktree.mkdir()
        self.git('init', '--quiet', '--separate-git-dir', str(m.STATE / 'migration-source.git'))
        self.cycle_commit = self.commit_files('cycle', self.allowed_files)
        self.layout = metal_persist.add_breenfs_layout(metal_persist.new_layout())
        self.cycle = dict(commit=self.cycle_commit, userland='v1', add='a',
                          boots=dict(add='a', migrate='b', readback='c'), layout=self.layout)
        self.migration.save(self.cycle)
        metal_persist.update_disk(layout=self.layout)
        self.backup = self.enterContext(patch.object(self.migration, 'backup'))

    def git(self, *args):
        return subprocess.check_output(['git', '-C', str(self.worktree), '-c', 'commit.gpgSign=false',
                                        '-c', 'core.hooksPath=/dev/null', '-c', 'user.name=Fixture',
                                        '-c', 'user.email=fixture@example.invalid', *args], text=True).strip()

    def commit_files(self, title, paths):
        for path in paths:
            file = self.worktree / path
            file.parent.mkdir(parents=True, exist_ok=True)
            file.write_text(title + '\n')
        self.git('add', '--', *paths)
        self.git('commit', '--quiet', '-m', title)
        return self.git('rev-parse', 'HEAD')

    def append_record(self, head, **fields):
        m.append_jsonl(m.STATE / 'migration-carry-forward.jsonl',
                       dict(cycleCommit=self.cycle_commit, headCommit=head,
                            allowedFiles=self.allowed_files) | fields)

    def admit(self, head):
        # Use a real Git range with fixture content under the ruling's cycle policy.
        with patch.object(self.migration, 'CARRY_FORWARD_CYCLE', self.cycle_commit, create=True):
            return self.migration.admit('cutover', 'next-boot', head, head)

    def test_carry_forward_admits_an_allowed_diff(self):
        head = self.commit_files('repair', self.allowed_files)
        self.append_record(head, ledgerLine=985)
        self.assertEqual(self.admit(head), self.cycle)
        self.backup.assert_called_once_with(self.layout, allowed={'a': 'add', 'b': 'migrate', 'c': 'readback'},
                                            active='next-boot')
        self.assertEqual(self.migration.state(), self.cycle)

    def test_carry_forward_refuses_files_outside_the_ruling(self):
        for path in ('scripts/metal/metal-boot', 'scripts/metal/metal_export.py',
                     'scripts/metal/migrate_manifest.py', 'scripts/metal/efi-stub/src/persist.rs',
                     'kernel/src/fs/breenfs/commit.rs', 'userspace/programs/src/persistent_migrate.rs'):
            with self.subTest(path=path):
                # Each comparison includes only one disallowed file.
                self.git('checkout', '--quiet', '--detach', self.cycle_commit)
                head = self.commit_files('outside ruling', [self.allowed_files[0], path])
                self.append_record(head, ledgerLine=985)
                with self.assertRaises(ValueError):
                    self.admit(head)
        self.backup.assert_not_called()

    def test_carry_forward_requires_the_ruling_line(self):
        head = self.commit_files('repair', self.allowed_files)
        self.append_record(head)
        with self.assertRaises(ValueError):
            self.admit(head)
        self.backup.assert_not_called()

    def test_slot_repair_carry_forward_admits_the_six_file_diff(self):
        head = self.commit_files('slot repair', self.slot_repair_files)
        self.append_record(head, ledgerLine=996, allowedFiles=self.slot_repair_files)
        self.assertEqual(self.admit(head), self.cycle)
        self.assertEqual(self.migration.state(), self.cycle)

    def test_slot_repair_carry_forward_requires_the_rulings_exact_list(self):
        head = self.commit_files('slot repair', self.slot_repair_files)
        self.append_record(head, ledgerLine=985, allowedFiles=self.slot_repair_files)
        self.append_record(head, ledgerLine=996, allowedFiles=self.allowed_files)
        with self.assertRaisesRegex(ValueError, 'matching carry-forward record'):
            self.admit(head)
        self.backup.assert_not_called()

    def test_slot_repair_carry_forward_refuses_an_outside_file(self):
        head = self.commit_files('outside slot ruling', self.slot_repair_files + ['scripts/metal/metal_export.py'])
        self.append_record(head, ledgerLine=996, allowedFiles=self.slot_repair_files)
        with self.assertRaisesRegex(ValueError, 'outside E24 ledger line 996'):
            self.admit(head)
        self.backup.assert_not_called()


if __name__ == '__main__':
    unittest.main()
