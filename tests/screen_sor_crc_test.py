"""U13: SOR proof has an explicit GK208-only secondary absence."""
import copy
import hashlib
import json
from pathlib import Path
import runpy
import struct
import unittest

import screens_approve_test as approval_fixture
from screen_perturb_acceptance_test import fixture

ROOT = Path(__file__).resolve().parents[1]
CODEC = runpy.run_path(str(ROOT / 'scripts/metal/screen-crc-export.py'))
SCREENS = runpy.run_path(str(ROOT / 'scripts/screen-verdict.py'))
ACCEPT = runpy.run_path(str(ROOT / 'scripts/screen-proof-acceptance.py'))
REASON = 'gk208-rg-no-words-u13'


def absent(record):
    record = copy.deepcopy(record)
    record['secondary_absent'] = REASON
    for sample in record['samples']:
        sample['crc2'] = None
    return record


def encode(record):
    names = [record[k].encode() for k in ('suite', 'case', 'checkpoint')]
    config = record['output_config']
    methods, samples = config['methods'], record['samples']
    header = bytearray(120)
    header[:8] = b'SCRCRC01'
    length = 120 + sum(map(len, names)) + 8 * len(methods) + 12 * len(samples)
    struct.pack_into('<III6H', header, 8, 2, length, record['backend'],
                     *map(len, names), len(methods), len(samples), 1)
    struct.pack_into('<4Q', header, 32, 7, 9, 7, 9)
    struct.pack_into('<14I', header, 64, 85000, 0, 1, 2, 1, 1280, 1024,
                     0, 112, 1280, 800, 0, 0, 1)
    return (bytes(header) + b''.join(names) +
            b''.join(struct.pack('<2I', *m) for m in methods) +
            b''.join(struct.pack('<3I', s['status'], int(s['crc'], 16),
                                 0 if s['crc2'] is None else int(s['crc2'], 16)) for s in samples))


class SorContract(unittest.TestCase):
    def setUp(self):
        self.helper = approval_fixture.ApproveTest()
        self.helper.setUp()
        self.addCleanup(self.helper.doCleanups)
        self.helper.run_import()
        self.boot, evidence = self.helper.native_fixture()
        self.record = absent(evidence['checkpoints'][0])
        self.evidence = dict(evidence, checkpoints=[self.record])
        self.serial = (self.boot / 'serial.txt').read_text().replace('crc2=abcdef12', 'crc2=none')

    def save(self, record=None, serial=None):
        (self.boot / 'screen-crc.json').write_text(json.dumps(dict(
            self.evidence, checkpoints=[self.record if record is None else record])))
        (self.boot / 'serial.txt').write_text(self.serial if serial is None else serial)

    def test_sor_export_and_native_approval(self):
        raw = encode(self.record)
        self.assertEqual(CODEC['decode'](raw), self.record)
        self.assertEqual(CODEC['validate_record'](self.record), ('12345678', 'none'))
        run = json.loads((self.boot / 'run.json').read_text())
        run['screenProof'] = True
        (self.boot / 'run.json').write_text(json.dumps(run))
        (self.boot / 'commit').write_text(run['commit'])
        label = 'crc-gpu-' + hashlib.sha256(b'draw/picture-0\0end').hexdigest()[:16]
        sha = hashlib.sha256(raw).hexdigest()
        (self.boot / ('diagnostic-' + label + '.bin')).write_bytes(raw)
        (self.boot / ('native-diagnostic-' + label + '.json')).write_text(json.dumps(
            dict(id=run['id'], sha256=sha, bytes=len(raw), complete=True)))
        anchor = f'SCREEN_CRC_UPLOAD gpu draw/picture-0 end label={label} bytes={len(raw)} sha256={sha} result=ok\n'
        serial = self.serial.replace('START cases=1', 'START cases=1 screen_proof=on')
        (self.boot / 'serial.txt').write_text(serial + anchor)
        self.assertEqual(CODEC['export'](self.boot)[0], [self.record])
        row = self.helper.run_import(native=self.boot)[0]['native'][0]
        self.assertEqual((row['backend'], row['crc'], row['crc2'], row['secondary_absent']),
                         (6, '12345678', 'none', REASON))
        manifest = dict(id='gpu', categories=[dict(id='draw', cases=[dict(id='picture-0')])])
        table = json.loads(self.helper.table.read_text())
        self.assertEqual(SCREENS['score'](manifest, serial, table, native=True,
                         config=row['config'], run=run)[0], 1)
        for backend in (4, 5, 6.0):
            with self.subTest(scorer_backend=backend), self.assertRaises(ValueError):
                SCREENS['score'](manifest, serial, table, native=True,
                                 config=row['config'], run=dict(run, backend=backend))
            with self.subTest(picture_backend=backend), self.assertRaises(ValueError):
                SCREENS['score'](manifest, serial, table, run=run, backend=backend)
        with self.assertRaises(ValueError):
            SCREENS['score'](manifest, serial, table, run=dict(run, backend=4), backend=6)
        with self.assertRaises(ValueError):
            SCREENS['score'](manifest, serial, table, run=run)
        for field, value in (('backend', 4), ('backend', 5), ('backend', 6.0),
                             ('secondary_absent', ''), ('secondary_absent', 'unknown')):
            bad = copy.deepcopy(table)
            bad['entries'][0]['native'][0][field] = value
            with self.subTest(row_field=field, value=value), self.assertRaises(ValueError):
                SCREENS['score'](manifest, serial, bad, native=True, config=row['config'], run=run)
        for secondary in ('00000000', 'garbage', 'abcdef12'):
            bad = serial.replace('crc2=none', 'crc2=' + secondary)
            with self.subTest(serial_secondary=secondary), self.assertRaises(ValueError):
                SCREENS['score'](manifest, bad, table, native=True, config=row['config'], run=run)
        bad = bytearray(raw)
        struct.pack_into('<I', bad, 116, 0)
        with self.assertRaises(ValueError): CODEC['decode'](bad)
        bad = bytearray(raw)
        struct.pack_into('<I', bad, len(bad) - 4, 0xabcdef12)
        with self.assertRaises(ValueError): CODEC['decode'](bad)

    def test_sor_absence_is_not_zero_garbage_or_a_foreign_backend(self):
        for backend in (4, 5):
            bad = dict(self.record, backend=backend)
            with self.subTest(backend=backend), self.assertRaises(ValueError):
                CODEC['validate_record'](bad)
            with self.assertRaises(ValueError): CODEC['decode'](encode(bad))
            self.save(bad)
            self.helper.run_import(native=self.boot, good=False)
        for value in ('00000000', 'garbage', 'abcdef12'):
            bad = copy.deepcopy(self.record)
            bad['samples'][2]['crc2'] = value
            with self.subTest(secondary=value), self.assertRaises(ValueError):
                CODEC['validate_record'](bad)
            self.save(bad)
            self.helper.run_import(native=self.boot, good=False)
        for reason in (None, '', 'unknown'):
            bad = copy.deepcopy(self.record)
            if reason is None: del bad['secondary_absent']
            else: bad['secondary_absent'] = reason
            with self.subTest(reason=reason), self.assertRaises(ValueError):
                CODEC['validate_record'](bad)
            self.save(bad)
            self.helper.run_import(native=self.boot, good=False)
        # claim-lint:ok: tests/screen_sor_crc_test.py exercises the U13 placeholder refusal.
        # Numeric zero in a legacy dual-source record is still not absence.
        legacy = copy.deepcopy(self.evidence['checkpoints'][0])
        legacy.pop('secondary_absent')
        for sample in legacy['samples']: sample['crc2'] = '00000000'
        with self.assertRaises(ValueError): CODEC['validate_record'](legacy)
        self.save(legacy, self.serial.replace('crc2=none', 'crc2=00000000'))
        self.helper.run_import(native=self.boot, good=False)

    def test_sor_phase_three_requires_sor_change_and_phase_four_restores_it(self):
        serial, table, records = fixture()
        serial = serial.replace('crc2=abcdef12', 'crc2=none')
        table['entries'][0]['native'][0].update(backend=6, crc2='none', secondary_absent=REASON)
        records = [absent(r) for r in records]
        self.assertEqual(ACCEPT['check_perturbation']('gpu', serial, table, records)['phases'], 4)
        for phase in (3, 4):
            bad = copy.deepcopy(records)
            old, new = ('87654321', '12345678') if phase == 3 else ('12345678', '87654321')
            for sample in bad[phase - 1]['samples']: sample['crc'] = new
            lines = serial.splitlines()
            lines[phase] = lines[phase].replace('crc=' + old, 'crc=' + new)
            with self.subTest(phase=phase), self.assertRaises(ValueError):
                ACCEPT['check_perturbation']('gpu', '\n'.join(lines), table, bad)
        # A secondary-only change cannot satisfy the SOR perturbation check.
        serial, table, records = fixture()
        for sample in records[2]['samples']:
            sample.update(crc='12345678', crc2='11111111')
        lines = serial.splitlines()
        lines[3] = lines[3].replace('crc=87654321', 'crc=12345678').replace('crc2=abcdef12', 'crc2=11111111')
        with self.assertRaises(ValueError):
            ACCEPT['check_perturbation']('gpu', '\n'.join(lines), table, records)

    def test_sor_live_grammar_and_uploaded_numeric_secondary_refusal(self):
        serial, _, _ = fixture()
        serial = serial.replace('crc2=abcdef12', 'crc2=none')
        serial = ('SUITE gpu START cases=1 screen_proof=on\n'
                  'SUITE gpu MEMORY before free=1000 heap_free_kb=500 user_pages=100\n'
                  'SUITE gpu CASE draw/one PASS ms=1\n' + serial +
                  'SUITE gpu DONE passed=1 failed=0 skipped=0 total=1\n'
                  'SUITE gpu MEMORY after free=1000 heap_free_kb=500 user_pages=100\n'
                  'SUITE gpu MEMORY reclaimed=PASS free_before=1000 free_after=1000 allowance=48\n')
        path = self.boot / 'serial.txt'
        path.write_text(serial)
        manifest = dict(id='gpu', categories=[dict(id='draw', cases=[dict(id='one')])])
        verdict = runpy.run_path(str(ROOT / 'scripts/suite-verdict.py'))['verdict']
        ok, why = verdict(manifest, path, [path])
        self.assertTrue(ok, why)
        # Convert the v2 envelope to v1: these are numeric secondary words,
        # without an absence reason. The rehashed upload must be refused.
        raw = bytearray(encode(self.record))
        del raw[116:120]
        struct.pack_into('<II', raw, 8, 1, len(raw))
        run = json.loads((self.boot / 'run.json').read_text())
        run['screenProof'] = True
        (self.boot / 'run.json').write_text(json.dumps(run))
        (self.boot / 'commit').write_text(run['commit'])
        label = 'crc-gpu-' + hashlib.sha256(b'draw/picture-0\0end').hexdigest()[:16]
        sha = hashlib.sha256(raw).hexdigest()
        (self.boot / ('diagnostic-' + label + '.bin')).write_bytes(raw)
        (self.boot / ('native-diagnostic-' + label + '.json')).write_text(json.dumps(
            dict(id=run['id'], sha256=sha, bytes=len(raw), complete=True)))
        path.write_text(f'SCREEN_CRC_UPLOAD gpu draw/picture-0 end label={label} bytes={len(raw)} sha256={sha} result=ok\n')
        with self.assertRaisesRegex(ValueError, 'unwritten CRC sample'):
            CODEC['export'](self.boot)

    def test_sor_r240_picture_gate_and_both_native_callers(self):
        serial, table, records = fixture()
        serial = ('SUITE gpu START cases=1 screen_proof=on\nSUITE gpu CASE draw/one PASS ms=1\n' +
                  serial.replace('crc2=abcdef12', 'crc2=none'))
        table['entries'][0]['native'][0].update(backend=6, crc2='none', secondary_absent=REASON)
        records = [absent(r) for r in records]
        baseline = [dict(records[0], checkpoint='end')]
        manifest = dict(id='gpu', categories=[dict(id='draw', cases=[dict(id='one')])])
        self.assertEqual(ACCEPT['r240_screens'](manifest, serial, table,
                         dict(screenProof=True), baseline, records)[0], 1)
        from gt710_resources_report_test import transcript, boot as gpu_boot
        from gt710_breengel_evidence_test import composition_capture
        from gt710_breengel_selection_test import boot as breengel_boot
        native = runpy.run_path(str(ROOT / 'scripts/metal/gt710-suite'))
        for suite, text, boot in [('gpu', transcript(all_pass=True), gpu_boot()),
                                  ('breengel', composition_capture(), breengel_boot())]:
            case = next(line.split()[3] for line in text.splitlines() if line.startswith(f'SUITE {suite} CASE '))
            end = (f'SUITE {suite} SCREEN {case} u13-observed sha256={"a" * 64} '
                   'mode=2x1 binding=2x1 crc=12345678 crc2=none raster=1280x1024 out=sor0\n')
            text = text.replace(f'SUITE {suite} DONE ', end + f'SUITE {suite} DONE ')
            with self.subTest(suite=suite):
                if suite == 'gpu': native['resources_suite'](text, boot)
                else: native['breengel_fronts_suite'](text, boot, 4)


if __name__ == '__main__':
    unittest.main()
