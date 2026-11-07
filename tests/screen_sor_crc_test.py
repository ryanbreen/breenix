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
        for backend in (4, 5):
            with self.subTest(scorer_backend=backend), self.assertRaises(ValueError):
                SCREENS['score'](manifest, serial, table, native=True,
                                 config=row['config'], run=dict(run, backend=backend))
        with self.assertRaises(ValueError):
            SCREENS['score'](manifest, serial, table, run=run)

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


if __name__ == '__main__':
    unittest.main()
