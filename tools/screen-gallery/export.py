#!/usr/bin/env python3
"""Export SCR1 disk or metal-boot evidence and serial inventory to Vigil candidates."""
import argparse
from fnmatch import fnmatchcase
import hashlib
import json
import os
from pathlib import Path
import re
import struct
import subprocess
import sys
import tempfile
import zlib

ROOT = Path(__file__).resolve().parents[2]
NAME = r'[a-z0-9]+(?:-[a-z0-9]+)*'
SCREEN = re.compile(rf'SUITE ({NAME}) SCREEN ({NAME}/{NAME}) ({NAME}) sha256=([0-9a-f]{{64}}) mode=([1-9][0-9]*x[1-9][0-9]*) binding=([1-9][0-9]*x[1-9][0-9]*)(?: crc=[0-9a-f]{{8}} crc2=[0-9a-f]{{8}} raster=[1-9][0-9]*x[1-9][0-9]* out=sor[0-9]+)?')
MAX_PIXELS = 2560 * 1600 * 4


def git(*args):
    return subprocess.check_output(['git', '-C', str(ROOT), *args], stderr=subprocess.PIPE, text=True).strip()


def evidence_rows(rows):
    return [{k: v for k, v in row.items() if k not in ('expect', 'expectCommit')} for row in rows]


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
    with (path if hasattr(path, 'read') else path.open('rb')) as f:
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


UPLOAD = re.compile(rf'SCREEN_UPLOAD ({NAME}) ({NAME}/{NAME}) ({NAME}) first=(0|[1-9][0-9]*) parts=([1-9][0-9]*) bytes=([1-9][0-9]*) sha256=([0-9a-f]{{64}})')


def known_pictures(manifest):
    if manifest is None:
        return {}
    from PIL import Image
    manifest = Path(manifest)
    doc = json.loads(manifest.read_text(encoding='utf-8'))
    if doc.get('version') != 1 or not isinstance(doc.get('rows'), list):
        raise ValueError('invalid known candidates manifest')
    pictures = {}
    for row in doc['rows']:
        digest, mode, name = row['sha256'], row['mode'], row['png']
        if not re.fullmatch('[0-9a-f]{64}', digest) or not re.fullmatch('[1-9][0-9]*x[1-9][0-9]*', mode):
            raise ValueError('invalid known picture identity')
        path = manifest.parent / name
        if path.resolve().parent != manifest.parent.resolve():
            raise ValueError('known PNG escapes candidates directory')
        width, height = map(int, mode.split('x'))
        if width * height * 4 > MAX_PIXELS:
            raise ValueError('known picture size exceeds bound')
        with Image.open(path) as image:
            if image.format != 'PNG' or image.size != (width, height):
                raise ValueError('known PNG mode mismatch')
            pixels = bytearray(image.convert('RGBA').tobytes('raw', 'BGRA'))
        pixels[3::4] = b'\xff' * (width * height)
        if hashlib.sha256(pixels).hexdigest() != digest:
            raise ValueError('known PNG pixel digest mismatch')
        pictures[digest] = (None, mode, bytes(pixels))
    return pictures


def metal_records(directory, known_manifest):
    import io
    directory = Path(directory)
    run = json.loads((directory / 'run.json').read_text())
    boot_id, commit = run['id'], run['commit']
    if not boot_id or not re.fullmatch('[0-9a-f]{40}', commit) or run.get('screenProof') is not True:
        raise ValueError('metal capture needs a proof run and commit')
    serial_path = directory / 'serial.txt'
    if not serial_path.exists():
        serial_path = directory / 'console.log'
    records = serial_records(serial_path)
    serial = serial_path.read_text()
    if re.search(r'^SCREEN_UPLOAD .*refused=', serial, re.M):
        raise ValueError('checkpoint picture upload refused')
    lookup = {(r[0], r[1], r[2]): r for r in records}
    order = {key: n for n, key in enumerate(lookup)}
    pictures, used, checkpoint_keys = {}, set(), set()
    next_part, last_checkpoint, total = {}, -1, 0
    for line in serial.splitlines():
        if not line.startswith('SCREEN_UPLOAD '):
            continue
        match = UPLOAD.fullmatch(line)
        if not match:
            raise ValueError('invalid checkpoint upload anchor')
        suite, case, checkpoint, first, parts, size, digest = match.groups()
        key = (suite, case, checkpoint)
        if key not in lookup or key in checkpoint_keys or order[key] <= last_checkpoint:
            raise ValueError('checkpoint upload order or identity differs')
        first, parts, size = map(int, (first, parts, size))
        if first != next_part.get(suite, 0) or size > MAX_PIXELS + 4096 or parts != (size + (2<<20) - 1) // (2<<20):
            raise ValueError('checkpoint upload parts differ')
        scope = directory / 'native-suites' / suite if (directory / 'native-suites').is_dir() else directory
        data = bytearray()
        for index in range(first, first + parts):
            path = scope / f'screen-part-{index:04}.bin'
            receipt = json.loads((scope / f'screen-part-upload-{index:04}.json').read_text())
            body = path.read_bytes()
            if (receipt.get('id') != boot_id or receipt.get('complete') is not True
                    or not 0 < len(body) <= 2<<20 or receipt.get('bytes') != len(body)
                    or receipt.get('sha256') != hashlib.sha256(body).hexdigest()):
                raise ValueError('checkpoint part receipt or digest differs')
            data.extend(body); used.add(path)
        total += len(data)
        if total > 64<<20 or len(data) != size or hashlib.sha256(data).hexdigest() != digest:
            raise ValueError('checkpoint upload aggregate digest differs')
        decoded = disk_records(io.BytesIO(b'SCRDISK1' + bytes(504) + data + bytes(512)))
        if len(decoded) != 1:
            raise ValueError('checkpoint upload record count differs')
        pixel_digest, picture = next(iter(decoded.items()))
        expected = lookup[key]
        if pixel_digest != expected[3] or picture[:2] != (key, expected[4]):
            raise ValueError('uploaded picture differs from serial digest or identity')
        pictures.setdefault(pixel_digest, picture)
        checkpoint_keys.add(key); last_checkpoint = order[key]; next_part[suite] = first + parts
    present = set(directory.glob('screen-part-*.bin')) | set(directory.glob('native-suites/*/screen-part-*.bin'))
    if present != used:
        raise ValueError('unanchored checkpoint part')
    # Uploaded observations take precedence; no known PNG can hide a bad upload.
    needed = {row[3] for row in records} - pictures.keys()
    if needed:
        known = known_pictures(known_manifest)
        for digest in needed:
            if digest not in known:
                raise ValueError('serial digest has no uploaded or known picture')
            pictures[digest] = known[digest]
    for suite, case, checkpoint, digest, mode, _ in records:
        if pictures[digest][1] != mode:
            raise ValueError('serial picture mode differs')
    return records, pictures, (boot_id, commit)


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
    if args.metal_boot:
        records, pictures, identity = metal_records(args.metal_boot, args.known_candidates)
        if (args.boot_id and args.boot_id != identity[0]) or (args.commit and args.commit != identity[1]):
            raise ValueError('metal run identity differs from arguments')
        args.boot_id, args.commit = identity
    else:
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
        if not args.metal_boot and pictures[digest][:2] != (identity, mode):
            raise ValueError(f'first occurrence mismatch: {identity}')
    if args.cases:
        patterns = args.cases.split(',')
        selected = []
        matched = set()
        for record in records:
            suite, case, checkpoint = record[:3]
            for pattern in patterns:
                identity = f'{suite}:{case}:{checkpoint}' if pattern.count(':') == 2 else f'{suite}:{case}'
                if pattern.count(':') not in (1, 2):
                    raise ValueError('--cases requires suite:category/case[:checkpoint] patterns')
                if fnmatchcase(identity, pattern):
                    matched.add(pattern)
                    if record not in selected:
                        selected.append(record)
        if matched != set(patterns):
            raise ValueError(f'unmatched case filters: {sorted(set(patterns) - matched)}')
        records = selected
    expect_commit = git('rev-parse', 'HEAD')
    manifest_path = args.manifests.resolve().relative_to(ROOT)
    manifests = {}
    rows = []
    for suite, case, checkpoint, digest, mode, binding in records:
        if suite not in manifests:
            doc = json.loads(git('show', f'{expect_commit}:{manifest_path}/{suite}.json'))
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
        expect_key = 'expectEnd' if checkpoint == 'end' and 'expectEnd' in item else 'expect'
        if expect_key in item:
            expect = item[expect_key]
            if not isinstance(expect, str) or not expect.strip() or len(expect) > 300:
                raise ValueError(f'invalid expect documentation: {suite} {case}')
            rows[-1].update(expect=expect, expectCommit=expect_commit)
    canonical = json.dumps({'version': 1, 'rows': evidence_rows(rows)}, sort_keys=True, separators=(',', ':'), ensure_ascii=False).encode()
    candidates_id = hashlib.sha256(canonical).hexdigest()
    args.output.mkdir(parents=True, exist_ok=True)
    target = args.output / candidates_id
    if args.refresh and not target.is_dir():
        raise ValueError(f'refresh requires existing candidates: {target}')
    if target.exists() and not args.refresh:
        raise ValueError(f'candidates already exist: {target}')
    with tempfile.TemporaryDirectory(prefix='.export-', dir=args.output) as temporary:
        directory = Path(temporary)
        for row in rows:
            width, height = map(int, row['mode'].split('x'))
            (directory / row['png']).write_bytes(png(width, height, pictures[row['sha256']][2]))
        (directory / 'manifest.json').write_text(json.dumps({'version': 1, 'candidates_id': candidates_id, 'rows': rows}, ensure_ascii=False, indent=2) + '\n', encoding='utf-8')
        if args.refresh:
            previous = json.loads((target / 'manifest.json').read_text(encoding='utf-8'))
            if (previous.get('version') != 1 or previous.get('candidates_id') != candidates_id
                    or evidence_rows(previous['rows']) != evidence_rows(rows)):
                raise ValueError('retained candidate evidence differs')
            for row in rows:
                if (target / row['png']).read_bytes() != (directory / row['png']).read_bytes():
                    raise ValueError(f'retained PNG differs: {row["png"]}')
            os.replace(directory / 'manifest.json', target / 'manifest.json')
        else:
            os.rename(directory, target)
    print(target)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('disk', type=Path, nargs='?'); parser.add_argument('serial', type=Path, nargs='?')
    parser.add_argument('--metal-boot', type=Path, help='read SCR1 uploads from a metal boot directory')
    parser.add_argument('--known-candidates', type=Path, help='reuse verified PNGs from this named candidates manifest')
    parser.add_argument('--boot-id'); parser.add_argument('--commit')
    parser.add_argument('--manifests', type=Path, default=ROOT / 'docs/suites')
    parser.add_argument('--cases', help='comma-separated suite:category/case[:checkpoint] patterns; validate the full capture before selecting rows')
    parser.add_argument('--refresh', action='store_true', help='update documentation in place, requiring unchanged evidence and PNGs')
    parser.add_argument('--output', type=Path, default=Path.home() / 'Library/Application Support/Vigil/breenix/screens/candidates')
    args = parser.parse_args()
    try:
        if args.metal_boot and (args.disk or args.serial):
            raise ValueError('--metal-boot replaces the disk and serial arguments')
        if not args.metal_boot and (not args.disk or not args.serial or not args.boot_id or not args.commit or not re.fullmatch(r'[0-9a-f]{40}', args.commit)):
            raise ValueError('boot id and 40-hex commit required')
        export(args)
    except (OSError, UnicodeError, ValueError, KeyError, TypeError, subprocess.SubprocessError) as exc:
        print(f'export failed: {exc}', file=sys.stderr)
        return 1
    return 0

if __name__ == '__main__':
    sys.exit(main())
