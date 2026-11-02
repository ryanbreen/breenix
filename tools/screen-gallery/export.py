#!/usr/bin/env python3
"""Export a stopped VM's SCR1 disk and serial inventory to Vigil candidates."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import struct
import sys
import tempfile
import zlib

ROOT = Path(__file__).resolve().parents[2]
NAME = r'[a-z0-9]+(?:-[a-z0-9]+)*'
SCREEN = re.compile(rf'SUITE ({NAME}) SCREEN ({NAME}/{NAME}) ({NAME}) sha256=([0-9a-f]{{64}}) mode=([1-9][0-9]*x[1-9][0-9]*) binding=([1-9][0-9]*x[1-9][0-9]*)(?: crc=[0-9a-f]{{8}} crc2=[0-9a-f]{{8}} raster=[1-9][0-9]*x[1-9][0-9]* out=sor[0-9]+)?')
MAX_PIXELS = 2560 * 1600 * 4


def serial_records(path):
    records, keys = [], set()
    for line in path.read_text(encoding='utf-8').splitlines():
        if not re.match(r'^SUITE \S+\s+SCREEN(?:\s|$)', line):
            continue
        match = SCREEN.fullmatch(line)
        if not match:
            raise ValueError(f'invalid SCREEN line: {line}')
        suite, case, checkpoint, digest, mode, binding = match.groups()
        key = (suite, case, checkpoint)
        if key in keys:
            raise ValueError(f'duplicate checkpoint: {key}')
        keys.add(key)
        w, h = map(int, mode.split('x')); bw, bh = map(int, binding.split('x'))
        if bw < w or bh < h:
            raise ValueError('binding smaller than mode')
        records.append((suite, case, checkpoint, digest, mode, binding))
    if not records:
        raise ValueError('serial has no SCREEN pictures')
    return records


def disk_records(path):
    pictures = {}
    with path.open('rb') as f:
        if f.read(512) != b'SCRDISK1' + bytes(504):
            raise ValueError('invalid disk marker')
        while True:
            header = f.read(80)
            if len(header) != 80:
                raise ValueError('truncated disk')
            if header == bytes(80):
                while chunk := f.read(1024 * 1024):
                    if any(chunk):
                        raise ValueError('data after disk end')
                break
            if header[:4] != b'SCR1':
                raise ValueError('invalid record magic')
            length, encoding, payload, width, height = struct.unpack_from('<IIIII', header, 4)
            sizes = struct.unpack_from('<HHH', header, 24)
            raw = width * height * 4
            if (not width or not height or raw > MAX_PIXELS or length < 80 or length > MAX_PIXELS + 4096
                    or length % 512 or struct.unpack_from('<I', header, 64)[0] != raw
                    or header[30:32] != bytes(2) or header[68:80] != bytes(12)
                    or any(n == 0 or n > 255 for n in sizes)):
                raise ValueError('invalid record header')
            rest = f.read(length - 80)
            if len(rest) != length - 80:
                raise ValueError('truncated record')
            start = sum(sizes); end = start + payload
            if end > len(rest) or (80 + end + 511) // 512 * 512 != length or any(rest[end:]):
                raise ValueError('invalid record padding')
            names = []; pos = 0
            for n, pattern in zip(sizes, (NAME, rf'{NAME}/{NAME}', NAME)):
                name = rest[pos:pos+n].decode('ascii'); pos += n
                if not re.fullmatch(pattern, name):
                    raise ValueError('invalid record name')
                names.append(name)
            data = rest[start:end]
            if encoding == 0 and payload == raw:
                pixels = data
            elif encoding == 1 and payload % 8 == 0 and payload < raw:
                pixels = bytearray()
                for pos in range(0, payload, 8):
                    count = struct.unpack_from('<I', data, pos)[0]
                    if not count or count > (raw - len(pixels)) // 4:
                        raise ValueError('invalid RLE run')
                    pixels.extend(data[pos+4:pos+8] * count)
                pixels = bytes(pixels)
            else:
                raise ValueError('invalid encoding')
            digest = header[32:64].hex()
            if len(pixels) != raw or any(pixels[i] != 255 for i in range(3, raw, 4)) or hashlib.sha256(pixels).hexdigest() != digest:
                raise ValueError('record pixel digest mismatch')
            if digest in pictures:
                raise ValueError('duplicate disk digest')
            pictures[digest] = (tuple(names), f'{width}x{height}', pixels)
    return pictures


def png(width, height, pixels):
    def chunk(kind, data):
        return struct.pack('>I', len(data)) + kind + data + struct.pack('>I', zlib.crc32(kind + data))
    rows = bytearray()
    for y in range(height):
        rows.append(0)
        for x in range(width):
            i = (y * width + x) * 4
            b, g, r, _ = pixels[i:i+4]
            rows.extend((r, g, b))
    return (b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', struct.pack('>IIBBBBB', width, height, 8, 2, 0, 0, 0))
            + chunk(b'IDAT', zlib.compress(rows)) + chunk(b'IEND', b''))


def export(args):
    records = serial_records(args.serial)
    pictures = disk_records(args.disk)
    first = {}
    for suite, case, checkpoint, digest, mode, binding in records:
        first.setdefault(digest, ((suite, case, checkpoint), mode))
    if set(first) != set(pictures):
        raise ValueError('serial and disk digest sets differ')
    for suite, case, checkpoint, digest, mode, binding in records:
        if pictures[digest][1] != mode:
            raise ValueError(f'mode mismatch: {suite} {case} {checkpoint}')
    for digest, (identity, mode) in first.items():
        if pictures[digest][:2] != (identity, mode):
            raise ValueError(f'first occurrence mismatch: {identity}')
    manifests = {}
    rows = []
    for suite, case, checkpoint, digest, mode, binding in records:
        if suite not in manifests:
            doc = json.loads((args.manifests / f'{suite}.json').read_text(encoding='utf-8'))
            if doc['id'] != suite:
                raise ValueError('manifest suite mismatch')
            manifests[suite] = {f"{c['id']}/{item['id']}": item for c in doc['categories'] for item in c['cases']}
        item = manifests[suite][case]
        title = item['title']
        section = item.get('contract_section', {'gpu': 'docs/gui/gpu.md §1.10', 'breengel': 'docs/gui/breengel.md §23.8'}.get(suite))
        if not isinstance(title, str) or not title or not isinstance(section, str) or not section:
            raise ValueError('missing title or contract section')
        rows.append(dict(suite=suite, case=case, title=title, contract_section=section,
                         checkpoint=checkpoint, mode=mode, binding=binding, sha256=digest,
                         boot_id=args.boot_id, commit=args.commit, intent=title,
                         png=f'{len(rows):05d}.png'))
    canonical = json.dumps({'version': 1, 'rows': rows}, sort_keys=True, separators=(',', ':'), ensure_ascii=False).encode()
    candidates_id = hashlib.sha256(canonical).hexdigest()
    args.output.mkdir(parents=True, exist_ok=True)
    target = args.output / candidates_id
    if target.exists():
        raise ValueError(f'candidates already exist: {target}')
    with tempfile.TemporaryDirectory(prefix='.export-', dir=args.output) as temporary:
        directory = Path(temporary)
        for row in rows:
            width, height = map(int, row['mode'].split('x'))
            (directory / row['png']).write_bytes(png(width, height, pictures[row['sha256']][2]))
        (directory / 'manifest.json').write_text(json.dumps({'version': 1, 'candidates_id': candidates_id, 'rows': rows}, ensure_ascii=False, indent=2) + '\n', encoding='utf-8')
        os.rename(directory, target)
    print(target)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('disk', type=Path); parser.add_argument('serial', type=Path)
    parser.add_argument('--boot-id', required=True); parser.add_argument('--commit', required=True)
    parser.add_argument('--manifests', type=Path, default=ROOT / 'docs/suites')
    parser.add_argument('--output', type=Path, default=Path.home() / 'Library/Application Support/Vigil/breenix/screens/candidates')
    args = parser.parse_args()
    try:
        if not args.boot_id or not re.fullmatch(r'[0-9a-f]{40}', args.commit):
            raise ValueError('boot id and 40-hex commit required')
        export(args)
    except (OSError, UnicodeError, ValueError, KeyError, TypeError) as exc:
        print(f'export failed: {exc}', file=sys.stderr)
        return 1
    return 0

if __name__ == '__main__':
    sys.exit(main())
