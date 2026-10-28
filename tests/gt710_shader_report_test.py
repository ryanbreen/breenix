"""Shader evidence is judged from the complete uploaded bytes and receipt."""
import copy
import hashlib
import json
from pathlib import Path
import runpy
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
API = runpy.run_path(str(ROOT / 'scripts/metal/gt710-suite'))
ASSETS = ROOT / 'kernel/assets/kepler'
SHA = lambda b: hashlib.sha256(b).hexdigest()


def fixture():
    manifest = json.loads((ASSETS / 'manifest.json').read_text())['files']
    assets = {s + '.' + ext: manifest['diagnostic-triangle-' + s + '.' + ext]['sha256']
              for s in ('vertex', 'fragment') for ext in ('bin', 'sph', 'json', 'asm')}
    segment = bytearray(640)
    for stage, offset in [('vertex', 48), ('fragment', 432)]:
        data = (ASSETS / ('diagnostic-triangle-' + stage + '.sph')).read_bytes() + (ASSETS / ('diagnostic-triangle-' + stage + '.bin')).read_bytes()
        segment[offset:offset + len(data)] = data
    flipped = bytearray(segment); flipped[0] ^= 1
    images = [bytes([85]) * 640, bytes(segment), bytes(flipped), bytes(segment)]
    rows = ''; readbacks = []
    for name, data in zip(('Withheld', 'AfterWithheld', 'Flipped', 'AfterFlipped'), images):
        for poison in (85, 170):
            readbacks.append(dict(case=name, poison=poison, bytes=640, sha256=SHA(data)))
            for offset in range(0, 640, 256):
                rows += f'GT710_SHADER_IMAGE {name} {poison} {offset} {data[offset:offset+256].hex()}\n'
    report = dict(schema=1, assets=assets, class_id=41367, subchannel=0, code_address=0x400000,
                  segment_bytes=640, offsets=[48, 432], gprs=[8, 4], readbacks=readbacks,
                  release=dict(target=0x500000, method=6924, operation=268496912, epoch=1, nonce=42,
                               observed=42, observed_epoch=1, consumed=True, accepted=True, elapsed_us=100),
                  trap_registers=[0x400100, 0x400108, 0x400110, 0x400118],
                  trap_before=[0]*4, trap_after=[0]*4, error=None, draw='pending-7.4')
    return report, rows


def serial(report, rows):
    return rows + 'GT710_SHADER ' + json.dumps(report) + '\n'


class ShaderReport(unittest.TestCase):
    def test_cases_and_mutations(self):
        report, rows = fixture()
        self.assertEqual([x['result'] for x in API['cases']('shaders', serial(report, rows))], ['PASS']*3)
        for edit, case in [(lambda r: r['gprs'].__setitem__(0, 9), 0),
                           (lambda r: r['trap_after'].__setitem__(0, 1), 2),
                           (lambda r: r['readbacks'][0].update(sha256=r['readbacks'][2]['sha256']), 1),
                           (lambda r: r['readbacks'][4].update(sha256=r['readbacks'][6]['sha256']), 1)]:
            bad = copy.deepcopy(report); edit(bad)
            self.assertEqual(API['cases']('shaders', serial(bad, rows))[case]['result'], 'FAIL')
        missing = '\n'.join(rows.splitlines()[1:]) + '\n'
        self.assertEqual(API['cases']('shaders', serial(report, missing))[1]['result'], 'FAIL')

    def test_bulk_rows_are_required_and_authenticated(self):
        report, rows = fixture()
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp)
            (path/'serial.txt').write_text(f'GT710_BULK {len(rows)} {SHA(rows.encode())}\nGT710_SHADER_BULK {len(rows)} {SHA(rows.encode())}\nGT710_SHADER {json.dumps(report)}\n')
            (path/'gt710-pixels.rle').write_text(rows)
            captured = API['capture_serial'](path)
            self.assertEqual(API['cases']('shaders', captured)[1]['result'], 'PASS')
            (path/'gt710-pixels.rle').write_text('')
            with self.assertRaises(ValueError): API['capture_serial'](path)

if __name__ == '__main__': unittest.main()
