"""Draw evidence is judged from authenticated complete images and fresh receipts."""
import copy
import hashlib
import json
from pathlib import Path
import runpy
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
API = runpy.run_path(str(ROOT / 'scripts/metal/gt710-suite'))
ORACLE = json.loads((ROOT / 'docs/gui/provenance/gt710-triangle.json').read_text())
SHA = lambda b: hashlib.sha256(b).hexdigest()
NAMES = ('triangle', 'vertex-control', 'after-vertex', 'fragment-control', 'after-fragment')


def fixture():
    image = bytearray(64 * 64 * 4)
    for y in range(64):
        for x in range(64):
            if x + y <= 31:
                image[(y * 64 + x) * 4:(y * 64 + x + 1) * 4] = bytes((20, 30, 40, 255))
    for s in ORACLE['samples']:
        i = (s['y'] * 64 + s['x']) * 4
        image[i:i+4] = bytes(s['bgra'])
    control = bytearray(image)
    for s in ORACLE['samples']:
        i = (s['y'] * 64 + s['x']) * 4
        control[i:i+4] = bytes((1, 2, 3, 255))
    images = {name: bytes(control if 'control' in name else image) for name in NAMES}
    images['present'] = bytes(image)
    records = []
    rows = ''
    for name in (*NAMES, 'present'):
        for poison in (85, 170):
            data = images[name]
            records.append(dict(case=name, poison=poison, bytes=len(data), sha256=SHA(data)))
            for off in range(0, len(data), 256):
                rows += f'GT710_DRAW_IMAGE {name} {poison} {off} {data[off:off+256].hex()}\n'
    receipt = dict(target=0x500000, epoch=2, nonce=42, observed_epoch=2, observed=42,
                   consumed=True, accepted=True)
    draw = dict(schema=1, class_id=0xa197, readbacks=records, release=receipt,
                trap_registers=[0x400100, 0x400108, 0x400110, 0x400118],
                trap_before=[0]*4, trap_after=[0]*4, error=None)
    present = dict(schema=1, width=64, height=64, pitch=256, background=0,
                   x=0, y=0, second_address=0x800000, active_address=0x800000,
                   firmware_writes=0, epoch=2, nonce=42)
    return draw, present, rows


def capture(draw, present, rows):
    return rows + 'GT710_DRAW ' + json.dumps(draw) + '\nGT710_PRESENT ' + json.dumps(present) + '\n'


class DrawReport(unittest.TestCase):
    def test_clean_and_mutations(self):
        draw, present, rows = fixture()
        def results(d=draw, p=present, r=rows):
            return [c['result'] for c in API['cases']('draw', capture(d, p, r))]
        self.assertEqual(results(), ['PASS'] * 3)
        for mutation in ('constant', 'dropped', 'changed', 'background', 'digest', 'stale'):
            d, p, r = copy.deepcopy(draw), copy.deepcopy(present), rows
            lines = r.splitlines(keepends=True)
            if mutation in ('constant', 'dropped', 'changed', 'background'):
                idx = next(i for i, line in enumerate(lines) if line.startswith('GT710_DRAW_IMAGE triangle 85 0 '))
                fields = lines[idx].split(); data = bytearray.fromhex(fields[4])
                if mutation == 'background': data[-4:] = b'\x01\x00\x00\x00'
                elif mutation == 'changed': data[0] ^= 1
                else: data[:] = bytes(len(data)) if mutation == 'dropped' else bytes((1, 2, 3, 255)) * (len(data)//4)
                lines[idx] = ' '.join(fields[:4]) + ' ' + data.hex() + '\n'; r = ''.join(lines)
            elif mutation == 'digest': d['readbacks'][0]['sha256'] = '0'*64
            else: p['active_address'] += 4096
            self.assertIn('FAIL', results(d, p, r), mutation)
        for name in ('vertex-control', 'fragment-control'):
            broken = rows.replace('GT710_DRAW_IMAGE '+name+' ', 'GT710_DRAW_IMAGE missing ')
            self.assertEqual(results(r=broken)[1], 'FAIL')

    def test_bulk_authentication(self):
        draw, present, rows = fixture(); data = rows.encode()
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp)
            (path/'gt710-pixels.rle').write_bytes(data)
            (path/'serial.txt').write_text(f'GT710_BULK {len(data)} {SHA(data)}\nGT710_DRAW_BULK {len(data)} {SHA(data)}\nGT710_DRAW {json.dumps(draw)}\nGT710_PRESENT {json.dumps(present)}\n')
            self.assertEqual([c['result'] for c in API['cases']('draw', API['capture_serial'](path))], ['PASS']*3)
            (path/'gt710-pixels.rle').write_bytes(data+b'x')
            with self.assertRaises(ValueError): API['capture_serial'](path)

if __name__ == '__main__': unittest.main()
