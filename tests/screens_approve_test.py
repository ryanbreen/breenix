"""Approval importer checks using synthetic decisions, pictures and native runs."""
import copy
import hashlib
import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

from PIL import Image

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / 'scripts/screens/approve.py'
spec = importlib.util.spec_from_file_location('screens_table', ROOT / 'tests/screens_table_test.py')
table_check = importlib.util.module_from_spec(spec)
spec.loader.exec_module(table_check)
spec = importlib.util.spec_from_file_location('screens_approve', SCRIPT)
approve = importlib.util.module_from_spec(spec)
spec.loader.exec_module(approve)


def canonical_id(rows):
    return hashlib.sha256(json.dumps({'version': 1, 'rows': rows}, sort_keys=True,
                          separators=(',', ':'), ensure_ascii=False).encode()).hexdigest()


class ApproveTest(unittest.TestCase):
    def test_vigil_entry_authors_and_marked_nondecisions(self):
        decision=self.decisions['entries'][0]
        decision.update(by='agent',evidence='Exact pixel check and independent Opus review match.',delegation=approve.DELEGATION)
        mark={k:self.rows[1][k] for k in approve.PICTURE}
        mark.update(state='investigating',issue='#2678',reason='Operator decision pending',marked_at='2026-11-07T14:00:00Z')
        self.decisions['entries']=[decision]
        self.decisions['non_decisions']=[mark]
        self.save_decisions()
        rows=self.run_import()
        self.assertEqual(len(rows),1)
        self.assertEqual(rows[0]['approval']['by'],'agent')
        self.assertEqual(rows[0]['approval']['evidence'],decision['evidence'])
        for bad in [dict(mark,state='approve'),dict(mark,sha256='invalid'),
                    dict(mark,sha256='e'*64),dict(mark,unexpected=1)]:
            self.decisions['non_decisions']=[bad];self.save_decisions();self.run_import(good=False)
        self.decisions['non_decisions']=[mark]
        decision['verdict']='reject';decision['reason']='Agent cannot reject'
        self.save_decisions();self.run_import(good=False)

    def test_utc_decision_works_with_python39_timestamp_parser(self):
        parser = approve.datetime

        class Python39Datetime:
            @staticmethod
            def fromisoformat(stamp):
                if stamp.endswith('Z'):
                    raise ValueError('Invalid isoformat string')
                return parser.fromisoformat(stamp)

        self.decisions['entries'][0]['decided_at'] = '2026-11-02T13:00:00Z'
        with patch.object(approve, 'datetime', Python39Datetime):
            approve.approval_export(self.decisions)

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.table = self.root / 'screens.json'
        self.table.write_text('{"version":1,"entries":[]}\n')
        self.pictures = self.root / 'candidates'
        self.pictures.mkdir()
        self.manifest = self.pictures / 'manifest.json'
        self.approval = self.root / 'decisions.json'
        self.rows = []
        for index, rgb in enumerate(((1, 2, 3), (4, 5, 6))):
            name = f'{index:05d}.png'
            Image.new('RGB', (2, 1), rgb).save(self.pictures / name)
            digest = hashlib.sha256(bytes((*reversed(rgb), 255)) * 2).hexdigest()
            self.rows.append(dict(suite='gpu', case=f'draw/picture-{index}', checkpoint='end',
                                 mode='2x1', binding='2x1', sha256=digest, title='Picture',
                                 contract_section='docs/gui/gpu.md §1.10', intent='Draw a picture',
                                 boot_id='capture-1', commit='a' * 40, png=name))
        self.save_candidates()
        self.decisions = {'candidates': canonical_id(self.rows), 'by': 'operator', 'entries': [
            {**{k: row[k] for k in ('suite', 'case', 'checkpoint', 'mode', 'sha256')},
             'verdict': 'approve' if i == 0 else 'reject', 'reason': '' if i == 0 else 'Wrong picture',
             'decided_at': '2026-11-02T08:00:00-05:00'} for i, row in enumerate(self.rows)]}
        self.save_decisions()

    def save_candidates(self):
        self.manifest.write_text(json.dumps({'version': 1, 'candidates_id': canonical_id(self.rows),
                                             'rows': self.rows}))

    def save_decisions(self):
        self.approval.write_text(json.dumps(self.decisions) + '\n')

    def run_import(self, native=None, good=True):
        before = self.table.read_bytes()
        saved = {p.name: p.read_bytes() for p in (self.root / 'approvals').glob('*')}
        cmd = [sys.executable, str(SCRIPT), '--table', str(self.table)]
        cmd += ['--native', str(native)] if native else [str(self.manifest), str(self.approval)]
        result = subprocess.run(cmd, capture_output=True, text=True)
        if good:
            self.assertEqual(result.returncode, 0, result.stderr + result.stdout)
            table_check.validate(self.table)
            return json.loads(self.table.read_text())['entries']
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertIn('approve failed:', result.stderr)
        self.assertEqual(self.table.read_bytes(), before)
        self.assertEqual({p.name: p.read_bytes() for p in (self.root / 'approvals').glob('*')}, saved)

    def test_documentation_preserves_candidate_identity(self):
        original = canonical_id(self.rows)
        for patch in ({'expect': 'A blue rectangle.'}, {'expectCommit': 'b' * 40},
                      {'expect': 'A blue rectangle.', 'expectCommit': 'b' * 40}):
            rows = copy.deepcopy(self.rows)
            rows[0].update(patch)
            self.manifest.write_text(json.dumps({'version': 1, 'candidates_id': original, 'rows': rows}))
            self.assertEqual(approve.candidates(self.manifest)[0], original)
            self.assertEqual(approve.candidates_id(rows), original)
        self.run_import()  # Existing operator decisions still import with documentation attached.
        for field, value in [('sha256', 'e' * 64), ('mode', '1x2'), ('case', 'draw/changed')]:
            with self.subTest(field=field):
                rows = copy.deepcopy(self.rows)
                rows[0][field] = value
                self.assertNotEqual(approve.candidates_id(rows), original)
                self.manifest.write_text(json.dumps({'version': 1, 'candidates_id': original, 'rows': rows}))
                with self.assertRaisesRegex(ValueError, 'candidates id mismatch'):
                    approve.candidates(self.manifest)
        rows = copy.deepcopy(self.rows)
        rows[0]['foo'] = 'unknown'
        self.manifest.write_text(json.dumps({'version': 1, 'candidates_id': approve.candidates_id(rows), 'rows': rows}))
        with self.assertRaisesRegex(ValueError, 'missing or unknown fields'):
            approve.candidates(self.manifest)

    def test_agent_import_preserves_evidence_and_forbids_rejection(self):
        delegation = ('can you have an agent do most of these approvals and validations? '
                      'most of these seem like they would be super easy for you to verify without my help.')
        self.decisions['by'] = 'agent'
        self.decisions['entries'] = [self.decisions['entries'][0]]
        decision = self.decisions['entries'][0]
        decision.update(evidence='Synthetic pixel check passes; independent review agrees.',
                        delegation=delegation)
        self.save_decisions()
        row = self.run_import()[0]
        self.assertEqual(row['approval']['by'], 'agent')
        for field in ('evidence', 'delegation'):
            self.assertEqual(row['approval'][field], decision[field])
        for field, value in [('evidence', ''), ('evidence', '  '), ('evidence', False),
                             ('delegation', ''), ('delegation', 'Delegated'),
                             ('verdict', 'reject')]:
            with self.subTest(field=field, value=value):
                original = decision[field]
                decision[field] = value
                self.save_decisions()
                self.run_import(good=False)
                decision[field] = original
        for field in ('evidence', 'delegation'):
            value = decision.pop(field)
            self.save_decisions()
            self.run_import(good=False)
            decision[field] = value

    def test_approve_reject_and_append_only_evidence(self):
        rows = self.run_import()
        self.assertEqual([r['case'] for r in rows], ['draw/picture-0'])
        self.assertEqual(rows[0]['approval']['capture_run'], 'capture-1')
        evidence = self.root / rows[0]['approval']['file']
        self.assertEqual(evidence.read_bytes(), self.approval.read_bytes())
        self.run_import()  # Idempotent import keeps the same evidence and table rows.
        self.assertEqual(len(list((self.root / 'approvals').glob('*.json'))), 1)
        previous = evidence.read_bytes()
        self.decisions['entries'][0]['reason'] = 'Reviewed again'
        self.save_decisions()
        rows = self.run_import()
        self.assertEqual(evidence.read_bytes(), previous)
        self.assertNotEqual(self.root / rows[0]['approval']['file'], evidence)

    def test_refuse_unapproved_or_non_candidate_decisions(self):
        original = copy.deepcopy(self.decisions)
        for field, value in [('candidates', 'f' * 64), ('by', 'agent')]:
            with self.subTest(field=field):
                self.decisions = copy.deepcopy(original)
                self.decisions[field] = value
                self.save_decisions()
                self.run_import(good=False)
        for patch in ({'sha256': 'e' * 64}, {'case': 'draw/absent'}, {'mode': '1x2'},
                      {'verdict': True}, {'verdict': 'approved'}, {'decided_at': 'yesterday'},
                      {'decided_at': '2026-11-02T08:00:00'}, {'unknown': 1}):
            with self.subTest(patch=patch):
                self.decisions = copy.deepcopy(original)
                self.decisions['entries'][0].update(patch)
                self.save_decisions()
                self.run_import(good=False)
        for field in ('by',):
            self.decisions = copy.deepcopy(original)
            del self.decisions[field]
            self.save_decisions()
            self.run_import(good=False)
        self.decisions = copy.deepcopy(original)
        del self.decisions['entries'][0]['decided_at']
        self.save_decisions()
        self.run_import(good=False)
        self.decisions = copy.deepcopy(original)
        self.decisions['entries'][1]['reason'] = ''
        self.save_decisions()
        self.run_import(good=False)
        self.decisions = copy.deepcopy(original)
        self.decisions['entries'].append(copy.deepcopy(original['entries'][0]))
        self.save_decisions()
        self.run_import(good=False)
        self.approval.write_text(self.approval.read_text().replace('"by": "operator"',
                                  '"by": "agent", "by": "operator"'))
        self.run_import(good=False)

    def test_reject_revokes_approved_picture_and_preserves_decision_bytes(self):
        self.run_import()
        directory, _ = self.native_fixture()
        self.run_import(native=directory)
        original = {p.name: p.read_bytes() for p in (self.root / 'approvals').glob('*.json')}
        decision = self.decisions['entries'][0]
        decision.update(verdict='reject', reason='Wrong picture after review')
        self.decisions['entries'] = [decision]
        self.save_decisions()
        self.assertEqual(self.run_import(), [])
        saved = {p.name: p.read_bytes() for p in (self.root / 'approvals').glob('*.json')}
        self.assertEqual(len(saved), 2)
        self.assertTrue(original.items() <= saved.items())
        self.assertIn(self.approval.read_bytes(), saved.values())

    def test_rejection_only_import_keeps_evidence_without_an_approved_row(self):
        self.decisions['entries'] = [self.decisions['entries'][1]]
        self.save_decisions()
        self.assertEqual(self.run_import(), [])
        saved = list((self.root / 'approvals').glob('*.json'))
        self.assertEqual(len(saved), 1)
        self.assertEqual(saved[0].read_bytes(), self.approval.read_bytes())

    def test_rejecting_another_picture_does_not_revoke_the_approved_digest(self):
        approved = self.run_import()[0]
        self.rows[0]['sha256'] = self.rows[1]['sha256']
        self.rows[0]['png'] = self.rows[1]['png']
        self.save_candidates()
        self.decisions['candidates'] = canonical_id(self.rows)
        decision = self.decisions['entries'][0]
        decision.update(sha256=self.rows[0]['sha256'], verdict='reject', reason='Different picture')
        self.decisions['entries'] = [decision]
        self.save_decisions()
        self.assertEqual(self.run_import(), [approved])

    def test_refuse_tampered_candidates_or_pixels(self):
        Image.new('RGB', (2, 1), (9, 8, 7)).save(self.pictures / '00000.png')
        self.run_import(good=False)
        Image.new('RGB', (2, 1), (1, 2, 3)).save(self.pictures / '00000.png')
        self.rows[0]['intent'] = 'Tampered manifest'
        self.manifest.write_text(json.dumps({'version': 1, 'candidates_id': self.decisions['candidates'],
                                             'rows': self.rows}))
        self.run_import(good=False)
        self.rows[0]['png'] = '../outside.png'
        self.save_candidates()
        self.decisions['candidates'] = canonical_id(self.rows)
        self.save_decisions()
        self.run_import(good=False)

    def test_replacement_discards_native_rows_for_old_picture(self):
        self.run_import()
        directory, evidence = self.native_fixture()
        self.run_import(native=directory)
        Image.new('RGB', (2, 1), (7, 8, 9)).save(self.pictures / '00000.png')
        self.rows[0]['sha256'] = hashlib.sha256(bytes((9, 8, 7, 255)) * 2).hexdigest()
        self.save_candidates()
        self.decisions['candidates'] = canonical_id(self.rows)
        self.decisions['entries'][0]['sha256'] = self.rows[0]['sha256']
        self.save_decisions()
        rows = self.run_import()
        self.assertNotIn('native', rows[0])
        self.assertEqual(len(list((self.root / 'approvals').glob('*.json'))), 2)

    def native_fixture(self):
        directory = self.root / 'native'
        directory.mkdir()
        run = {'id': '20261102T140000Z-metal', 'platform': 'metal', 'arch': 'x86_64',
               'mode': 'suite', 'commit': 'b' * 40, 'evidenceEligible': True}
        (directory / 'run.json').write_text(json.dumps(run))
        row = self.rows[0]
        (directory / 'serial.txt').write_text(
            'SUITE gpu START cases=1\n'
            'SUITE gpu CASE draw/picture-0 PASS ms=1\n'
            f'SUITE gpu SCREEN draw/picture-0 end sha256={row["sha256"]} mode=2x1 binding=2x1 '
            'crc=12345678 crc2=abcdef12 raster=1280x1024 out=sor0\n')
        config = dict(mode='2x1', raster='1280x1024', viewport=[0, 112, 1280, 800],
                      out='sor0', sources=['sor0', 'rg0'], methods=[[0x404, 0], [0x4d0, 0]])
        binding = {'resource': 7, 'present_fence': 9}
        capture = {**{k: row[k] for k in ('suite', 'case', 'checkpoint')}, 'backend': 6,
                   'output_config': config, 'binding_before': binding, 'binding_after': binding,
                   'elapsed_us': 85000, 'disarmed': True, 'display_error': 0, 'status': 1,
                   'samples': [{'status': 1, 'crc': '12345678', 'crc2': 'abcdef12'} for _ in range(5)]}
        evidence = {'version': 1, 'run': run['id'], 'commit': run['commit'], 'checkpoints': [capture]}
        (directory / 'screen-crc.json').write_text(json.dumps(evidence))
        return directory, evidence

    def test_native_rejects_disagreeing_proof_modes(self):
        self.run_import()
        directory, _ = self.native_fixture()
        run_path = directory / 'run.json'
        run = json.loads(run_path.read_text())
        run['screenProof'] = True
        run_path.write_text(json.dumps(run))
        self.run_import(native=directory, good=False)
        run['screenProof'] = False
        run_path.write_text(json.dumps(run))
        serial = directory / 'serial.txt'
        serial.write_text(serial.read_text().replace('START cases=1', 'START cases=1 screen_proof=on'))
        self.run_import(native=directory, good=False)

    def test_native_refuses_unwritten_zero_crc_entries(self):
        self.run_import()
        directory, evidence = self.native_fixture()
        for sample in evidence['checkpoints'][0]['samples']:
            sample['crc'] = '00000000'
        (directory / 'screen-crc.json').write_text(json.dumps(evidence))
        serial = directory / 'serial.txt'
        serial.write_text(serial.read_text().replace('crc=12345678', 'crc=00000000'))
        self.run_import(native=directory, good=False)

    def test_native_requires_matched_parent_and_stable_capture(self):
        self.run_import()
        directory, evidence = self.native_fixture()
        original = copy.deepcopy(evidence)
        capture = evidence['checkpoints'][0]
        expected_config = hashlib.sha256(json.dumps(capture['output_config'], sort_keys=True,
                                separators=(',', ':'), ensure_ascii=False).encode()).hexdigest()
        rows = self.run_import(native=directory)
        self.assertEqual(rows[0]['native'], [dict(out='sor0', raster='1280x1024', config=expected_config,
                              crc='12345678', crc2='abcdef12', derived_from_run=evidence['run'], commit='b'*40)])
        self.run_import(native=directory)
        changes = [('status', 3), ('status', 0), ('elapsed_us', 250001), ('disarmed', False),
                   ('display_error', 1), ('backend', 4), ('samples', capture['samples'][:4]),
                   ('binding_after', {'resource': 8, 'present_fence': 9})]
        for field, value in changes:
            with self.subTest(field=field, value=value):
                bad = copy.deepcopy(original)
                bad['checkpoints'][0][field] = value
                (directory / 'screen-crc.json').write_text(json.dumps(bad))
                self.run_import(native=directory, good=False)
        for field, value in [('crc', '99999999'), ('crc2', '99999999'), ('status', 0)]:
            bad = copy.deepcopy(original)
            bad['checkpoints'][0]['samples'][2][field] = value
            (directory / 'screen-crc.json').write_text(json.dumps(bad))
            self.run_import(native=directory, good=False)
        (directory / 'screen-crc.json').write_text(json.dumps(original))
        serial = directory / 'serial.txt'
        original_serial = serial.read_text()
        for replacement in (original_serial.replace(self.rows[0]['sha256'], 'f'*64),
                            original_serial.replace('mode=2x1', 'mode=1x2'),
                            original_serial.replace('crc=12345678', 'crc=99999999'),
                            original_serial.replace('draw/picture-0', 'draw/absent'),
                            original_serial + original_serial):
            serial.write_text(replacement)
            self.run_import(native=directory, good=False)
        serial.write_text(original_serial)
        run = json.loads((directory / 'run.json').read_text())
        run['platform'] = 'qemu'
        (directory / 'run.json').write_text(json.dumps(run))
        self.run_import(native=directory, good=False)


if __name__ == '__main__':
    unittest.main()
