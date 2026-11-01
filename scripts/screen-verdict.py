#!/usr/bin/env python3
"""Compare SCREEN records with the shared approved screen table.

Native scoring requires --config with the output configuration digest. Host
captures are digest files at DIR/SUITE/CATEGORY/CASE/CHECKPOINT.sha256.
Each supplied capture must name a serial checkpoint and match its digest.
"""
import argparse
import json
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
NAME = r'[a-z0-9]+(?:-[a-z0-9]+)*'
CASE = rf'{NAME}/{NAME}'
MODE = r'[1-9][0-9]*x[1-9][0-9]*'
HEX64 = r'[0-9a-f]{64}'
RECORD = re.compile(
    rf'SUITE (?P<suite>{NAME}) SCREEN (?P<case>{CASE}) (?P<checkpoint>{NAME}) '
    rf'(?:error=(?P<error>{NAME})|sha256=(?P<sha>{HEX64}) mode=(?P<mode>{MODE})'
    rf' binding=(?P<binding>{MODE})'
    rf'(?: crc=(?P<crc>[0-9a-f]{{8}}) crc2=(?P<crc2>[0-9a-f]{{8}})'
    rf' raster=(?P<raster>{MODE}) out=(?P<out>sor[0-9]+))?)\Z')


def score(manifest, serial, table, native=False, config=None, captures=None):
    suite = manifest['id']
    cases = {f"{category['id']}/{case['id']}" for category in manifest['categories']
             for case in category['cases']}
    approved = {}
    for entry in table['entries']:
        if entry['suite'] == suite:
            key = (entry['case'], entry['checkpoint'])
            if key in approved or key[0] not in cases:
                raise ValueError(f'invalid or duplicate table entry {key}')
            approved[key] = entry
    if table['version'] != 1:
        raise ValueError('unsupported table version')
    records = {}
    for line in serial.splitlines():
        if not re.match(rf'^SUITE {re.escape(suite)}\s+SCREEN(?:\s|$)', line):
            continue
        match = RECORD.fullmatch(line)
        if not match:
            raise ValueError(f'malformed SCREEN line: {line}')
        record = match.groupdict()
        if record['binding']:
            w, h = map(int, record['mode'].split('x'))
            bw, bh = map(int, record['binding'].split('x'))
            if bw < w or bh < h:
                raise ValueError('binding smaller than mode')
        key = (record['case'], record['checkpoint'])
        if key[0] not in cases:
            raise ValueError(f'SCREEN names unknown case {key[0]}')
        if key in records:
            raise ValueError(f'duplicate checkpoint {key[0]} {key[1]}')
        if record['error']:
            raise ValueError(f'checkpoint {key[0]} {key[1]} error={record["error"]}')
        records[key] = record
    for key, entry in approved.items():
        if key not in records:
            raise ValueError(f'missing checkpoint {key[0]} {key[1]}')
        record = records[key]
        if record['sha'] != entry['sha256'] or record['mode'] != entry['mode']:
            raise ValueError(f'digest or mode mismatch for {key[0]} {key[1]}')
        if native:
            rows = [row for row in entry.get('native', [])
                    if row['config'] == config and row['out'] == record['out']
                    and row['raster'] == record['raster']]
            if (not record['crc'] or len(rows) != 1 or
                    any(record[field] != rows[0][field] for field in ('crc', 'crc2'))):
                raise ValueError(f'native CRC or configuration mismatch for {key[0]} {key[1]}')
    if captures is not None:
        if not captures.is_dir():
            raise ValueError(f'host captures directory not found: {captures}')
        for path in sorted(captures.rglob('*.sha256')):
            parts = path.relative_to(captures).parts
            if len(parts) != 4 or parts[0] != suite:
                raise ValueError(f'invalid host capture path: {path}')
            key = (f'{parts[1]}/{parts[2]}', path.stem)
            digest = path.read_text(encoding='utf-8').strip()
            if key not in records or digest != records[key]['sha']:
                raise ValueError(f'host capture mismatch for {key[0]} {key[1]}')
    extra = sorted(records.keys() - approved.keys())
    return len(approved), extra


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('manifest', type=Path)
    parser.add_argument('serial_log', type=Path)
    parser.add_argument('--table', type=Path, default=ROOT / 'docs/suites/frames/screens.json')
    parser.add_argument('--native', action='store_true')
    parser.add_argument('--config', help='SHA-256 of native output configuration (required with --native)')
    parser.add_argument('--host-captures', type=Path)
    args = parser.parse_args()
    try:
        if args.native and (not args.config or not re.fullmatch(HEX64, args.config)):
            raise ValueError('--native requires a 64-hex --config')
        manifest = json.loads(args.manifest.read_text(encoding='utf-8'))
        table = json.loads(args.table.read_text(encoding='utf-8'))
        count, extra = score(manifest, args.serial_log.read_text(encoding='utf-8'), table,
                             args.native, args.config, args.host_captures)
    except (OSError, UnicodeError, ValueError, KeyError, TypeError) as exc:
        print(f'SCREENS FAIL: {exc}')
        return 1
    print(f'SCREENS PASS approved={count} matched={count} unapproved={len(extra)}')
    for case, checkpoint in extra:
        print(f'UNAPPROVED {case} {checkpoint}')
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
