#!/usr/bin/env python3
"""Import Vigil operator decisions or derive native rows from a stopped run.

See README.md for the approval export and screen-crc.json evidence formats.
PNG verification uses Pillow; no guest code or display access is involved.
"""
import argparse
from datetime import datetime
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import sys
import tempfile

from PIL import Image

ROOT = Path(__file__).resolve().parents[2]
HEX64 = r'[0-9a-f]{64}'
HEX40 = r'[0-9a-f]{40}'
HEX8 = r'[0-9a-f]{8}'
NAME = r'[a-z0-9]+(?:-[a-z0-9]+)*'
MODE = r'[1-9][0-9]*x[1-9][0-9]*'
IDENTITY = {'suite', 'case', 'checkpoint'}
PICTURE = IDENTITY | {'mode', 'sha256'}


def require(ok, message):
    if not ok:
        raise ValueError(message)


def matches(pattern, value):
    return type(value) is str and re.fullmatch(pattern, value) is not None


def fields(obj, required, optional=()):
    require(type(obj) is dict and required <= obj.keys() <= required | set(optional),
            'missing or unknown fields')


def nonempty(value):
    return type(value) is str and bool(value.strip())


def uint(value, positive=False):
    return type(value) is int and int(positive) <= value <= 0xffffffff


def parse_json(data):
    def unique(pairs):
        obj = {}
        for key, value in pairs:
            require(key not in obj, 'duplicate JSON field')
            obj[key] = value
        return obj
    return json.loads(data, object_pairs_hook=unique)


def read_json(path):
    return parse_json(path.read_text(encoding='utf-8'))


def digest(obj):
    return hashlib.sha256(json.dumps(obj, sort_keys=True, separators=(',', ':'),
                                     ensure_ascii=False, allow_nan=False).encode()).hexdigest()


def identity(row):
    require(matches(NAME, row['suite']) and matches(f'{NAME}/{NAME}', row['case']) and
            matches(NAME, row['checkpoint']), 'invalid checkpoint identity')
    return tuple(row[k] for k in ('suite', 'case', 'checkpoint'))


def picture(row):
    key = identity(row)
    require(matches(MODE, row['mode']) and matches(HEX64, row['sha256']), 'invalid picture fields')
    return key


def indexed(rows, check):
    require(type(rows) is list, 'expected array')
    result = {}
    for row in rows:
        key = check(row)
        require(key not in result, 'duplicate checkpoint')
        result[key] = row
    return result


def approval_export(doc):
    fields(doc, {'candidates', 'by', 'entries'})
    require(matches(HEX64, doc['candidates']) and doc['by'] == 'operator',
            'approval must name candidates and be by operator')

    def decision(row):
        fields(row, PICTURE | {'verdict', 'decided_at'}, {'reason'})
        key = picture(row)
        require(row['verdict'] in ('approve', 'reject'), 'invalid verdict')
        require('reason' not in row or type(row['reason']) is str, 'invalid reason')
        require(row['verdict'] != 'reject' or nonempty(row.get('reason')), 'reject requires reason')
        stamp = row['decided_at']
        require(matches(r'\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:\d{2})', stamp),
                'decision time must be timezone-qualified ISO 8601')
        require(datetime.fromisoformat(stamp).utcoffset() is not None, 'missing decision timezone')
        return key
    return indexed(doc['entries'], decision)


def candidates(path):
    doc = read_json(path)
    fields(doc, {'version', 'candidates_id', 'rows'})
    require(type(doc['version']) is int and doc['version'] == 1, 'invalid candidates version')
    require(matches(HEX64, doc['candidates_id']) and
            digest({'version': 1, 'rows': doc['rows']}) == doc['candidates_id'], 'candidates id mismatch')

    def candidate(row):
        fields(row, PICTURE | {'title', 'contract_section', 'binding', 'boot_id', 'commit', 'intent', 'png'})
        key = picture(row)
        require(all(nonempty(row[k]) for k in ('title', 'contract_section', 'boot_id', 'intent')) and
                matches(HEX40, row['commit']), 'invalid capture metadata')
        require(matches(MODE, row['binding']), 'invalid binding')
        w, h = map(int, row['mode'].split('x'))
        bw, bh = map(int, row['binding'].split('x'))
        require(bw >= w and bh >= h and w * h <= 2560 * 1600, 'invalid picture dimensions')
        name = row['png']
        require(matches(r'[a-zA-Z0-9][a-zA-Z0-9._-]*\.png', name), 'invalid PNG path')
        png = path.parent / name
        require(png.resolve().parent == path.parent.resolve(), 'PNG escapes candidates directory')
        with Image.open(png) as image:
            require(image.format == 'PNG' and image.size == (w, h) and image.mode == 'RGB',
                    'expected opaque RGB PNG at candidate mode')
            # Pillow emits RGB; the shared digest hashes BGRX with X forced to FF.
            pixels = image.tobytes('raw', 'BGRX')
            canonical = bytearray(pixels)
            canonical[3::4] = b'\xff' * (w * h)
            require(hashlib.sha256(canonical).hexdigest() == row['sha256'], 'PNG pixel digest mismatch')
        return key
    return doc['candidates_id'], indexed(doc['rows'], candidate)


def native_row(row):
    fields(row, {'out', 'raster', 'config', 'crc', 'derived_from_run', 'commit'}, {'crc2'})
    require(matches(r'sor[0-9]+', row['out']) and matches(MODE, row['raster']) and
            matches(HEX64, row['config']) and matches(HEX8, row['crc']) and
            ('crc2' not in row or matches(HEX8, row['crc2'])) and
            nonempty(row['derived_from_run']) and matches(HEX40, row['commit']), 'invalid native row')
    return row['out'], row['raster'], row['config']


def validate_table(table, path, pending=None):
    """Validate preserved rows and saved evidence before mutating the table."""
    fields(table, {'version', 'entries'}, {'about'})
    require(type(table['version']) is int and table['version'] == 1, 'invalid table version')
    require('about' not in table or nonempty(table['about']), 'invalid about')

    def entry(row):
        fields(row, PICTURE | {'intent', 'approval'}, {'native'})
        key = picture(row)
        require(nonempty(row['intent']), 'invalid intent')
        a = row['approval']
        fields(a, {'by', 'file', 'candidates', 'capture_run', 'capture_commit'})
        require(a['by'] == 'operator' and matches(HEX64, a['candidates']) and
                nonempty(a['capture_run']) and matches(HEX40, a['capture_commit']), 'invalid approval metadata')
        require(matches(r'approvals/[a-zA-Z0-9][a-zA-Z0-9._-]*\.json', a['file']), 'invalid approval path')
        evidence_path = path.parent / a['file']
        require(evidence_path.resolve().parent == (path.parent / 'approvals').resolve(),
                'approval path escapes directory')
        evidence = pending[a['file']] if pending and a['file'] in pending else read_json(evidence_path)
        require(type(evidence) is dict and evidence.get('candidates') == a['candidates'] and
                type(evidence.get('entries')) is list, 'approval candidates mismatch')
        require(any(type(item) is dict and all(item.get(k) == row[k] for k in
                    (*IDENTITY, 'sha256')) and item.get('verdict') == 'approve'
                    for item in evidence['entries']), 'digest not approved')
        require(type(row.get('native', [])) is list, 'invalid native array')
        keys = [native_row(item) for item in row.get('native', [])]
        require(len(set(keys)) == len(keys), 'duplicate native configuration')
        return key
    return indexed(table['entries'], entry)


def import_decisions(table, path, manifest, approval):
    candidate_id, rows = candidates(manifest)
    # Preserve exactly the bytes read here, rather than reserializing the decisions.
    raw = approval.read_bytes()
    doc = parse_json(raw.decode('utf-8'))
    decisions = approval_export(doc)
    require(doc['candidates'] == candidate_id, 'approval candidates mismatch')
    entries = validate_table(table, path)
    for key, decision in decisions.items():
        require(key in rows and all(decision[k] == rows[key][k] for k in PICTURE),
                'decision is not in candidates')
    filename = f'approvals/approval-{hashlib.sha256(raw).hexdigest()}.json'
    for key, decision in decisions.items():
        if decision['verdict'] != 'approve':
            previous = entries.get(key)
            if previous and all(previous[k] == decision[k] for k in ('sha256', 'mode')):
                del entries[key]
            continue
        row = rows[key]
        entry = {k: row[k] for k in PICTURE | {'intent'}}
        entry['approval'] = dict(by='operator', file=filename, candidates=candidate_id,
                                 capture_run=row['boot_id'], capture_commit=row['commit'])
        previous = entries.get(key)
        if previous and previous['sha256'] == row['sha256'] and previous['mode'] == row['mode'] and 'native' in previous:
            entry['native'] = previous['native']
        entries[key] = entry
    table['entries'] = [entries[key] for key in sorted(entries)]
    validate_table(table, path, {filename: doc})
    return filename, raw


def derive_native(table, path, directory):
    entries = validate_table(table, path)
    run = read_json(directory / 'run.json')
    require(type(run) is dict and run.get('platform') == 'metal' and run.get('arch') == 'x86_64' and
            run.get('mode') == 'suite' and run.get('evidenceEligible') is True and
            nonempty(run.get('id')) and matches(HEX40, run.get('commit')), 'not an eligible native suite run')
    evidence = read_json(directory / 'screen-crc.json')
    fields(evidence, {'version', 'run', 'commit', 'checkpoints'})
    require(type(evidence['version']) is int and evidence['version'] == 1 and
            evidence['run'] == run['id'] and evidence['commit'] == run['commit'], 'native run identity mismatch')
    # Use the existing serial grammar without changing the shared scorer.
    spec = importlib.util.spec_from_file_location('screen_verdict', ROOT / 'scripts/screen-verdict.py')
    scorer = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(scorer)
    records = {}
    reported = set()
    for line in (directory / 'serial.txt').read_text(encoding='utf-8').splitlines():
        case = re.match(rf'^SUITE ({NAME}) CASE ({NAME}/{NAME}) ', line)
        if case:
            reported.add((case[1], case[2]))
        if not re.match(r'^SUITE \S+\s+SCREEN(?:\s|$)', line):
            continue
        match = scorer.RECORD.fullmatch(line)
        require(match is not None, 'malformed native SCREEN')
        record = match.groupdict()
        key = tuple(record[k] for k in ('suite', 'case', 'checkpoint'))
        require(key not in records and key[:2] in reported, 'duplicate SCREEN or SCREEN before CASE')
        records[key] = record
    require(type(evidence['checkpoints']) is list and bool(evidence['checkpoints']), 'missing native checkpoints')
    seen = set()
    for capture in evidence['checkpoints']:
        fields(capture, IDENTITY | {'backend', 'output_config', 'binding_before', 'binding_after',
                                    'elapsed_us', 'disarmed', 'display_error', 'status', 'samples'})
        key = identity(capture)
        require(key not in seen, 'duplicate native checkpoint')
        seen.add(key)
        require(key in entries and key in records, 'native checkpoint has no approved parent or SCREEN')
        parent, record = entries[key], records[key]
        require(matches(MODE, record['binding']), 'missing native scanout binding')
        w, h = map(int, record['mode'].split('x'))
        bw, bh = map(int, record['binding'].split('x'))
        require(bw >= w and bh >= h, 'native binding smaller than mode')
        require(record['sha'] == parent['sha256'] and record['mode'] == parent['mode'],
                'native parent did not match in this run')
        require(type(capture['backend']) is int and capture['backend'] == 6 and
                uint(capture['elapsed_us']) and capture['elapsed_us'] <= 250000 and
                capture['disarmed'] is True and type(capture['display_error']) is int and
                capture['display_error'] == 0, 'invalid native capture outcome')
        # Context status: MIT crc907d.c, pinned in THIRD_PARTY_NOTICES.
        # Bit 0 is completion; bits 1..5 are overflow.
        require(uint(capture['status']) and capture['status'] & 1 and not capture['status'] & 0x3e,
                'unfinished or overflowed CRC context')
        for binding in (capture['binding_before'], capture['binding_after']):
            fields(binding, {'resource', 'present_fence'})
            require(all(type(v) is int and 0 < v <= 0xffffffffffffffff for v in binding.values()),
                    'invalid pinned binding')
        require(capture['binding_before'] == capture['binding_after'], 'binding moved during capture')
        config = capture['output_config']
        fields(config, {'mode', 'raster', 'viewport', 'out', 'sources', 'methods'})
        require(config['mode'] == record['mode'] and config['raster'] == record['raster'] and
                config['out'] == record['out'] and matches(MODE, config['raster']) and
                matches(r'sor[0-9]+', config['out']), 'native output configuration mismatch')
        require(type(config['sources']) is list and len(config['sources']) == 2 and
                config['sources'][0] == config['out'] and matches(r'rg[0-9]+', config['sources'][1]),
                'invalid CRC sources')
        viewport = config['viewport']
        require(type(viewport) is list and len(viewport) == 4 and all(uint(v) for v in viewport) and
                viewport[2] > 0 and viewport[3] > 0, 'invalid viewport')
        rw, rh = map(int, config['raster'].split('x'))
        require(viewport[0] + viewport[2] <= rw and viewport[1] + viewport[3] <= rh, 'viewport outside raster')
        methods = config['methods']
        require(type(methods) is list and bool(methods) and all(type(m) is list and len(m) == 2 and
                all(uint(v) for v in m) and m[0] % 4 == 0 for m in methods), 'invalid output methods')
        require([m[0] for m in methods] == sorted({m[0] for m in methods}), 'output methods must be unique and sorted')
        samples = capture['samples']
        require(type(samples) is list and 5 <= len(samples) <= 255, 'requires first written entry plus K=4 samples')
        for sample in samples:
            fields(sample, {'status', 'crc', 'crc2'})
            require(type(sample['status']) is int and sample['status'] == 1 and
                    matches(HEX8, sample['crc']) and sample['crc'] != '00000000' and
                    matches(HEX8, sample['crc2']), 'unwritten or invalid CRC entry')
        require(all(sample['crc'] == record['crc'] and sample['crc2'] == record['crc2']
                    for sample in samples[1:]), 'unstable CRC or serial CRC mismatch')
        native = dict(out=config['out'], raster=config['raster'], config=digest(config),
                      crc=record['crc'], crc2=record['crc2'], derived_from_run=run['id'], commit=run['commit'])
        native_key = native_row(native)
        parent['native'] = [row for row in parent.get('native', []) if native_row(row) != native_key] + [native]
    validate_table(table, path)


def atomic_write(path, data):
    temporary = None
    try:
        with tempfile.NamedTemporaryFile(dir=path.parent, prefix='.screens-', delete=False) as out:
            temporary = Path(out.name)
            out.write(data)
            out.flush()
            os.fsync(out.fileno())
        os.replace(temporary, path)
    finally:
        if temporary is not None:
            temporary.unlink(missing_ok=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('candidates', nargs='?', type=Path)
    parser.add_argument('approval', nargs='?', type=Path)
    parser.add_argument('--table', type=Path, default=ROOT / 'docs/suites/frames/screens.json')
    parser.add_argument('--native', type=Path, metavar='RUN_DIRECTORY')
    args = parser.parse_args()
    try:
        require((args.native is not None and args.candidates is None and args.approval is None) or
                (args.native is None and args.candidates is not None and args.approval is not None),
                'supply candidates and approval, or --native RUN_DIRECTORY')
        table = read_json(args.table)
        saved = None
        if args.native:
            derive_native(table, args.table, args.native)
        else:
            saved = import_decisions(table, args.table, args.candidates, args.approval)
        data = (json.dumps(table, indent=2, sort_keys=True, ensure_ascii=False, allow_nan=False) + '\n').encode()
        if saved:
            filename, raw = saved
            approval_path = args.table.parent / filename
            approval_path.parent.mkdir(exist_ok=True)
            try:
                with approval_path.open('xb') as out:
                    out.write(raw)
                    out.flush()
                    os.fsync(out.fileno())
            except FileExistsError:
                require(approval_path.read_bytes() == raw, 'existing approval evidence differs')
        atomic_write(args.table, data)
        print(f'SCREENS UPDATED entries={len(table["entries"])}')
    except (OSError, UnicodeError, ValueError, KeyError, TypeError, Image.DecompressionBombError) as exc:
        print(f'approve failed: {exc}', file=sys.stderr)
        return 1
    return 0


if __name__ == '__main__':
    sys.exit(main())
