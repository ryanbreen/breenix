#!/usr/bin/env python3
"""Validate the shared screen table and its operator approval evidence."""
import json
import re
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
TABLE = ROOT / 'docs/suites/frames/screens.json'
HEX64 = re.compile(r'[0-9a-f]{64}\Z')
HEX40 = re.compile(r'[0-9a-f]{40}\Z')
HEX8 = re.compile(r'[0-9a-f]{8}\Z')
NAME = re.compile(r'[a-z0-9]+(?:-[a-z0-9]+)*\Z')
CASE = re.compile(r'[a-z0-9]+(?:-[a-z0-9]+)*/[a-z0-9]+(?:-[a-z0-9]+)*\Z')
MODE = re.compile(r'[1-9][0-9]*x[1-9][0-9]*\Z')


def require(condition, message):
    if not condition:
        raise ValueError(message)


def fields(obj, required, optional=()):
    require(type(obj) is dict, 'expected object')
    require(required <= obj.keys() and obj.keys() <= required | set(optional),
            'missing or unknown fields')


def matches(pattern, value):
    return type(value) is str and pattern.fullmatch(value) is not None


def nonempty(value):
    return type(value) is str and bool(value.strip())


def read_json(path):
    def unique(pairs):
        result = {}
        for key, value in pairs:
            require(key not in result, 'duplicate JSON field')
            result[key] = value
        return result
    try:
        return json.loads(path.read_text(encoding='utf-8'), object_pairs_hook=unique)
    except (OSError, UnicodeError, json.JSONDecodeError) as exc:
        raise ValueError(f'{path}: {exc}') from exc


def validate(table_path):
    table = read_json(table_path)
    fields(table, {'version', 'entries'}, {'about'})
    require(type(table['version']) is int and table['version'] == 1, 'invalid version')
    require('about' not in table or nonempty(table['about']), 'invalid about')
    require(type(table['entries']) is list, 'entries must be an array')
    seen = set()
    for row in table['entries']:
        fields(row, {'suite', 'case', 'checkpoint', 'mode', 'sha256', 'intent', 'approval'}, {'native'})
        require(matches(NAME, row['suite']) and matches(CASE, row['case']) and
                matches(NAME, row['checkpoint']), 'invalid checkpoint identity')
        key = (row['suite'], row['case'], row['checkpoint'])
        require(key not in seen, 'duplicate checkpoint')
        seen.add(key)
        require(matches(MODE, row['mode']) and matches(HEX64, row['sha256']) and
                nonempty(row['intent']), 'invalid picture fields')
        approval = row['approval']
        fields(approval, {'by', 'file', 'candidates', 'capture_run', 'capture_commit'})
        require(approval['by'] == 'operator', 'approval is not by operator')
        require(matches(HEX64, approval['candidates']) and nonempty(approval['capture_run']) and
                matches(HEX40, approval['capture_commit']), 'invalid approval metadata')
        filename = approval['file']
        require(type(filename) is str and re.fullmatch(r'approvals/[a-zA-Z0-9][a-zA-Z0-9._-]*\.json', filename) is not None,
                'invalid approval path')
        approval_dir = (table_path.parent / 'approvals').resolve()
        path = table_path.parent / filename
        require(path.resolve().parent == approval_dir and path.is_file(), 'missing approval file')
        evidence = read_json(path)
        require(type(evidence) is dict and evidence.get('candidates') == approval['candidates'] and
                type(evidence.get('entries')) is list, 'approval candidates mismatch or missing entries')
        require(any(type(item) is dict and all(item.get(k) == row[k] for k in
                    ('suite', 'case', 'checkpoint', 'sha256')) and item.get('verdict') == 'approve'
                    for item in evidence['entries']), 'digest not approved')
        native = row.get('native', [])
        require(type(native) is list, 'native must be an array')
        native_keys = set()
        for item in native:
            fields(item, {'out', 'raster', 'config', 'crc', 'derived_from_run', 'commit'}, {'crc2'})
            require(type(item['out']) is str and re.fullmatch(r'sor[0-9]+', item['out']) is not None and
                    matches(MODE, item['raster']) and matches(HEX64, item['config']) and
                    matches(HEX8, item['crc']) and ('crc2' not in item or matches(HEX8, item['crc2'])) and
                    nonempty(item['derived_from_run']) and matches(HEX40, item['commit']), 'invalid native row')
            native_key = (item['out'], item['raster'], item['config'])
            require(native_key not in native_keys, 'duplicate native configuration')
            native_keys.add(native_key)


class ScreensTableTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.dir = Path(self.tmp.name)
        (self.dir / 'approvals').mkdir()
        self.path = self.dir / 'screens.json'
        self.digest = 'a' * 64
        self.candidates = 'b' * 64
        self.row = {'suite': 'gpu', 'case': 'present/example', 'checkpoint': 'end',
                    'mode': '2560x1600', 'sha256': self.digest, 'intent': 'Example',
                    'approval': {'by': 'operator', 'file': 'approvals/one.json',
                                 'candidates': self.candidates, 'capture_run': 'run',
                                 'capture_commit': 'c' * 40}}
        self.evidence = {'candidates': self.candidates, 'entries': [
            {'suite': 'gpu', 'case': 'present/example', 'checkpoint': 'end',
             'sha256': self.digest, 'verdict': 'approve'}]}

    def write(self, entries):
        self.path.write_text(json.dumps({'version': 1, 'entries': entries}))

    def approve(self):
        (self.dir / 'approvals/one.json').write_text(json.dumps(self.evidence))

    def test_empty_and_approved(self):
        self.write([])
        validate(self.path)
        self.approve()
        self.write([self.row])
        validate(self.path)

    def test_missing_or_inconsistent_approval(self):
        self.write([self.row])
        with self.assertRaises(ValueError):
            validate(self.path)
        self.evidence['entries'][0]['sha256'] = 'd' * 64
        self.approve()
        with self.assertRaises(ValueError):
            validate(self.path)
        self.evidence['entries'][0]['sha256'] = self.digest
        self.evidence['candidates'] = 'e' * 64
        self.approve()
        with self.assertRaises(ValueError):
            validate(self.path)

    def test_operator_and_strict_fields(self):
        self.approve()
        self.row['approval']['by'] = 'agent'
        self.write([self.row])
        with self.assertRaises(ValueError):
            validate(self.path)
        self.row['approval']['by'] = 'operator'
        self.row['extra'] = True
        self.write([self.row])
        with self.assertRaises(ValueError):
            validate(self.path)

    def test_native_requires_approved_parent(self):
        self.row['native'] = [{'out': 'sor0', 'raster': '1280x1024', 'config': 'f' * 64,
                               'crc': '12345678', 'crc2': 'abcdef12',
                               'derived_from_run': 'run', 'commit': 'c' * 40}]
        self.approve()
        self.write([self.row])
        validate(self.path)
        self.evidence['entries'][0]['verdict'] = 'reject'
        self.approve()
        with self.assertRaises(ValueError):
            validate(self.path)

    def test_repository_table(self):
        validate(TABLE)


if __name__ == '__main__':
    unittest.main()
