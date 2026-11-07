#!/usr/bin/env python3
"""Export uploaded SCRCRC01 records from a retained metal boot."""
import hashlib
import json
from pathlib import Path
import re
import struct
import sys

ANCHOR = re.compile(r'SCREEN_CRC_UPLOAD (gpu|breengel) ([a-z0-9-]+/[a-z0-9-]+) ([a-z0-9-]+) label=(crc-(?:gpu|breengel)-[0-9a-f]{16}) bytes=([0-9]+) sha256=([0-9a-f]{64}) result=ok')

def decode(data):
    if len(data) < 116 or len(data) >= 4096 or data[:8] != b'SCRCRC01':
        raise ValueError('invalid CRC header')
    version, length, backend, a, b, c, nm, ns, disarmed = struct.unpack_from('<III6H', data, 8)
    if version != 1 or length != len(data) or backend != 6 or not all(0 < n <= 255 for n in (a,b,c)) or nm > 64 or ns > 255 or disarmed > 1 or 116+a+b+c+8*nm+12*ns != length:
        raise ValueError('invalid CRC extent')
    rb, fb, ra, fa = struct.unpack_from('<4Q', data, 32)
    elapsed, error, status, mw, mh, rw, rh, x, y, w, h, sor, rg = struct.unpack_from('<13I', data, 64)
    at = 116
    names = []
    for n in (a,b,c):
        names.append(data[at:at+n].decode('utf-8')); at += n
    methods = []
    for _ in range(nm):
        methods.append(list(struct.unpack_from('<2I', data, at))); at += 8
    samples = []
    for _ in range(ns):
        st, crc, crc2 = struct.unpack_from('<3I', data, at); at += 12
        samples.append(dict(status=st, crc=f'{crc:08x}', crc2=f'{crc2:08x}'))
    return dict(zip(('suite','case','checkpoint'), names), backend=backend,
        binding_before=dict(resource=rb, present_fence=fb), binding_after=dict(resource=ra, present_fence=fa),
        elapsed_us=elapsed, disarmed=bool(disarmed), display_error=error, status=status,
        output_config=dict(mode=f'{mw}x{mh}', raster=f'{rw}x{rh}', viewport=[x,y,w,h],
                           out=f'sor{sor}', sources=[f'sor{sor}',f'rg{rg}'], methods=methods), samples=samples)

PHASE_CHECKPOINTS = ('perturb-one', 'perturb-two', 'perturb-three', 'perturb-four')


def validate_record(record):
    """Collection eligibility, independently checked from the uploaded samples."""
    def require(ok, reason):
        if not ok: raise ValueError(reason)
    def uint(value): return type(value) is int and 0 <= value <= 0xffffffff
    require(record['backend'] == 6 and type(record['backend']) is int and
            uint(record['elapsed_us']) and record['elapsed_us'] <= 250000 and
            record['disarmed'] is True and record['display_error'] == 0 and
            uint(record['display_error']) and uint(record['status']) and
            record['status'] & 1 and not record['status'] & 0x3e, 'invalid CRC collection outcome')
    for binding in (record['binding_before'], record['binding_after']):
        require(set(binding) == {'resource', 'present_fence'} and
                all(type(v) is int and 0 < v < 1 << 64 for v in binding.values()), 'invalid CRC binding')
    require(record['binding_before'] == record['binding_after'], 'CRC binding moved')
    c = record['output_config']
    require(set(c) == {'mode', 'raster', 'viewport', 'out', 'sources', 'methods'} and
            all(re.fullmatch(r'[1-9][0-9]*x[1-9][0-9]*', c[k]) for k in ('mode', 'raster')) and
            re.fullmatch(r'sor[0-9]+', c['out']), 'invalid CRC configuration')
    require(type(c['sources']) is list and len(c['sources']) == 2 and c['sources'][0] == c['out'] and
            re.fullmatch(r'rg[0-9]+', c['sources'][1]), 'invalid CRC sources')
    v = c['viewport']; rw, rh = map(int, c['raster'].split('x'))
    require(type(v) is list and len(v) == 4 and all(uint(n) for n in v) and min(v[2:]) > 0 and
            v[0] + v[2] <= rw and v[1] + v[3] <= rh, 'invalid CRC viewport')
    methods = c['methods']
    require(type(methods) is list and 0 < len(methods) <= 64 and all(type(m) is list and len(m) == 2 and
            all(uint(n) for n in m) and m[0] % 4 == 0 for m in methods) and
            [m[0] for m in methods] == sorted({m[0] for m in methods}), 'invalid CRC methods')
    samples = record['samples']
    require(type(samples) is list and 5 <= len(samples) <= 255, 'missing CRC samples')
    for sample in samples:
        require(set(sample) == {'status', 'crc', 'crc2'} and type(sample['status']) is int and sample['status'] == 1 and
                all(re.fullmatch(r'[0-9a-f]{8}', sample[k]) for k in ('crc', 'crc2')) and
                sample['crc'] != '00000000', 'unwritten CRC sample')
    require(all((s['crc'], s['crc2']) == (samples[1]['crc'], samples[1]['crc2']) for s in samples[1:]),
            'unstable CRC samples')
    return samples[1]['crc'], samples[1]['crc2']


def export(boot):
    run = json.loads((boot/'run.json').read_text())
    if run.get('screenProof') is not True or not run.get('id') or not re.fullmatch(r'[0-9a-f]{40}', run.get('commit', '')):
        raise ValueError('invalid proof run identity')
    if (boot/'commit').read_text().strip() != run['commit']:
        raise ValueError('boot commit mismatch')
    serial_path = boot/'serial.txt'
    if not serial_path.exists(): serial_path = boot/'console.log'
    serial = serial_path.read_text(encoding='utf-8')
    records, perturbations = [], []
    seen = set()
    for line in serial.splitlines():
        if not line.startswith('SCREEN_CRC_UPLOAD '): continue
        match = ANCHOR.fullmatch(line)
        if not match: raise ValueError('invalid CRC upload anchor')
        suite, case, checkpoint, label, size, digest = match.groups()
        key = (suite, case, checkpoint)
        expected = 'crc-' + suite + '-' + hashlib.sha256((case+'\0'+checkpoint).encode()).hexdigest()[:16]
        if key in seen or label != expected: raise ValueError('duplicate or mismatched CRC identity')
        seen.add(key)
        path = boot/('diagnostic-'+label+'.bin')
        if not path.exists(): path = boot/'native-suites'/suite/('diagnostic-'+label+'.bin')
        receipt = json.loads((path.parent/('native-diagnostic-'+label+'.json')).read_text())
        if receipt.get('id') != run['id'] or receipt.get('sha256') != digest or receipt.get('bytes') != int(size) or receipt.get('complete') is not True:
            raise ValueError('CRC receipt mismatch')
        data = path.read_bytes()
        if len(data) != int(size) or hashlib.sha256(data).hexdigest() != digest: raise ValueError('CRC upload digest mismatch')
        record = decode(data)
        if tuple(record[k] for k in ('suite','case','checkpoint')) != key: raise ValueError('CRC record identity mismatch')
        (perturbations if checkpoint in PHASE_CHECKPOINTS else records).append(record)
    if not records: raise ValueError('no checkpoint CRC uploads')
    if serial_path != boot/'serial.txt': (boot/'serial.txt').write_text(serial, encoding='utf-8')
    (boot/'screen-crc.json').write_text(json.dumps(dict(version=1, run=run['id'], commit=run['commit'], checkpoints=records), indent=2)+'\n')
    (boot/'screen-perturb.json').write_text(json.dumps(dict(version=1, run=run['id'], commit=run['commit'], checkpoints=perturbations), indent=2)+'\n')
    return records, perturbations

if __name__ == '__main__':
    try:
        if len(sys.argv) != 2: raise ValueError('usage: screen-crc-export.py BOOT_DIR')
        export(Path(sys.argv[1]))
    except (OSError, ValueError, KeyError, TypeError, UnicodeError, struct.error) as exc:
        print(f'CRC export failed: {exc}', file=sys.stderr); sys.exit(1)
