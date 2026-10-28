"""Judge M7.5 synthetic captures against u1's rational image oracle."""
import copy
from fractions import Fraction
import hashlib
import json
from pathlib import Path
import runpy
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
API = runpy.run_path(str(ROOT / 'scripts/metal/gt710-suite'))
ORACLE = runpy.run_path(str(ROOT / 'tests/gt710_contexts_oracle_test.py'))
V1 = json.loads((ROOT / 'docs/gui/provenance/gt710-triangle.json').read_text(), parse_float=Fraction)
V2 = json.loads((ROOT / 'docs/gui/provenance/gt710-contexts.json').read_text(), parse_float=Fraction)['v2']
SHA = lambda data: hashlib.sha256(data).hexdigest()
STEPS = (('a-v1', 'a', 'v1'), ('b-gradient', 'b', 'gradient'),
         ('a-v2', 'a', 'v2'), ('b-restored', 'b', 'gradient'),
         ('suppress', 'a', 'v2'), ('after-suppress', 'a', 'v2'),
         ('corrupt', 'b', 'gradient'), ('after-corrupt', 'b', 'gradient'))


def triangle(oracle):
    _, weights = ORACLE['raster'](oracle['vertices'])
    data = bytearray(16384)
    for (x, y), w in weights.items():
        data[(y*64+x)*4:(y*64+x+1)*4] = bytes(ORACLE['colour'](oracle['vertices'], w)[1])
    return bytes(data)


def fixture():
    assets = json.loads((ROOT / 'kernel/assets/kepler/manifest.json').read_text())['files']
    channels = [dict(id=c, instance=0x100000+i*0x10000, image=0x120000+i*0x20000,
                     target=0x200000+i*0x100000, epoch=2+i, release=0x400000+i*4096,
                     vertex_sha256=assets['diagnostic-triangle-vertex.bin']['sha256'],
                     fragment_sha256=assets[fragment+'.bin']['sha256'])
                for i, (c, fragment) in enumerate((('a', 'diagnostic-triangle-fragment'),
                                                  ('b', 'gradient-fragment')))]
    gradient = bytes(v for y in range(64) for x in range(64) for v in (4*x, 4*y, 2*(x+y), 255))
    images = dict(v1=triangle(V1), v2=triangle(V2), gradient=gradient)
    bad = bytearray(gradient); bad[0] ^= 1
    state = dict(a=bytes(16384), b=bytes(16384))
    report = dict(schema=1, class_id=0xa197, channels=channels, frames=[], readbacks=[],
                  current_register=0x409b00, trap_registers=[0x400100, 0x400108, 0x400110, 0x400118],
                  error=None)
    captured = {}
    for i, (name, channel, variant) in enumerate(STEPS):
        c = channels[channel == 'b']; peer = 'b' if channel == 'a' else 'a'
        data = bytes(16384) if name == 'suppress' else bytes(bad) if name == 'corrupt' else images[variant]
        current = 0x80000000 | (c['instance'] >> 12)
        report['frames'].append(dict(case=name, channel=channel, variant=variant,
            current_before=current, current_after=current, trap_before=[0]*4, trap_after=[0]*4,
            peer_before=[SHA(state[peer])]*2, peer_after=[SHA(state[peer])]*2,
            accepted=name not in ('suppress', 'corrupt'),
            receipt=dict(target=c['release'], epoch=c['epoch'], nonce=100+i,
                         observed_epoch=c['epoch'], observed=100+i, consumed=True, accepted=True)))
        state[channel] = data
        for poison in (85, 170):
            captured[name, poison] = data
    return report, captured


def capture(report, images):
    report = copy.deepcopy(report)
    rows, report['readbacks'] = [], []
    for (name, poison), data in images.items():
        report['readbacks'].append(dict(case=name, poison=poison, bytes=len(data), sha256=SHA(data)))
        for y in range(64):
            runs = []
            for x in range(64):
                word = int.from_bytes(data[(y*64+x)*4:(y*64+x+1)*4], 'little')
                if runs and runs[-1][1] == word:
                    runs[-1][0] += 1
                else:
                    runs.append([1, word])
            rows.append(f'GT710_CTX_IMAGE {name} {poison} {y} ' +
                        ','.join(f'{count:x}:{word:08x}' for count, word in runs) + '\n')
    return 'GT710_CONTEXTS ' + json.dumps(report) + '\n', ''.join(rows)


class ContextsReport(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.report, cls.images = fixture()

    def results(self, report, images):
        serial, rows = capture(report, images)
        return API['cases']('gr-contexts', rows + serial)

    def test_clean_capture_and_definition(self):
        self.assertEqual([r['result'] for r in self.results(self.report, self.images)], ['PASS', 'PASS'])
        API['validate_manifest'](json.loads((ROOT / 'docs/suites/gt710.json').read_text()))
        effort = json.loads((ROOT / 'docs/efforts/metal.json').read_text())
        milestone = next(m for e in effort['efforts'] if e['id'] == 'metal-gt710'
                         for m in e['milestones'] if m['id'] == 'gr-contexts')
        self.assertEqual(milestone['measure'], dict(suite='gt710', categories=['gr-contexts']))
        scorer = runpy.run_path(str(ROOT / 'scripts/metal/metal_score.py'))
        self.assertEqual(scorer['host_milestone']('gt710-gr-contexts.json'), 'metal-gt710/gr-contexts')

    def test_binding_peer_receipt_and_control_mutations(self):
        for mutation in ('other-context', 'peer', 'peer-both', 'accepted', 'successor',
                         'epoch-nonce', 'stale', 'alias', 'shader', 'trap', 'error'):
            with self.subTest(mutation=mutation):
                r = copy.deepcopy(self.report)
                frames = r['frames']
                if mutation == 'other-context': frames[2]['current_after'] = frames[1]['current_after']
                elif mutation == 'peer': frames[2]['peer_after'][0] = '0'*64
                elif mutation == 'peer-both': frames[2]['peer_before'] = frames[2]['peer_after'] = ['0'*64]*2
                elif mutation == 'accepted': frames[4]['accepted'] = True
                elif mutation == 'successor': frames[5]['case'] = 'missing'
                elif mutation == 'epoch-nonce': frames[2]['receipt'] = copy.deepcopy(frames[0]['receipt'])
                elif mutation == 'stale': frames[2]['receipt']['observed_epoch'] -= 1
                elif mutation == 'alias': r['channels'][1]['target'] = r['channels'][0]['target']
                elif mutation == 'shader': r['channels'][1]['fragment_sha256'] = r['channels'][0]['fragment_sha256']
                elif mutation == 'trap': frames[2]['trap_after'][0] = 1
                else: r['error'] = 'preempt timeout recovered'
                self.assertIn('FAIL', [c['result'] for c in self.results(r, self.images)])

    def test_authenticated_pixel_and_poison_mutations(self):
        for name, offset in (('b-gradient', 0), ('a-v1', (V1['samples'][0]['y']*64+V1['samples'][0]['x'])*4),
                             ('a-v2', (V2['samples'][0]['y']*64+V2['samples'][0]['x'])*4),
                             ('a-v2', 0), ('b-restored', 500), ('after-suppress', 0), ('after-corrupt', 0)):
            images = dict(self.images)
            for poison in (85, 170):
                data = bytearray(images[name, poison]); data[offset] ^= 1
                images[name, poison] = bytes(data)
            self.assertIn('FAIL', [r['result'] for r in self.results(self.report, images)], name)
        for control, successor in (('suppress', 'after-suppress'), ('corrupt', 'after-corrupt')):
            images = dict(self.images)
            for poison in (85, 170): images[control, poison] = images[successor, poison]
            self.assertEqual(self.results(self.report, images)[1]['result'], 'FAIL', control)
        images = dict(self.images); del images['a-v2', 170]
        self.assertIn('FAIL', [r['result'] for r in self.results(self.report, images)])
        serial, rows = capture(self.report, self.images)
        serial = serial.replace(SHA(self.images['a-v1', 85]), '0'*64, 1)
        self.assertIn('FAIL', [r['result'] for r in API['cases']('gr-contexts', rows+serial)])

    def test_bulk_authentication_and_serial_exclusion(self):
        serial, rows = capture(self.report, self.images)
        data = rows.encode('ascii')
        anchor = f'GT710_CTX_BULK {len(data)} {SHA(data)}\n'
        overall = f'GT710_BULK {len(data)} {SHA(data)}\n'
        with tempfile.TemporaryDirectory() as tmp:
            p = Path(tmp)
            (p/'gt710-pixels.rle').write_bytes(data)
            (p/'serial.txt').write_text(anchor + serial + overall)
            self.assertEqual([r['result'] for r in API['cases']('gr-contexts', API['capture_serial'](p))], ['PASS']*2)
            for bad in (anchor.replace(SHA(data), '0'*64) + serial + overall,
                        serial + overall, anchor + serial + rows + overall):
                (p/'serial.txt').write_text(bad)
                with self.assertRaises(ValueError): API['capture_serial'](p)
            (p/'serial.txt').write_text(anchor + serial + overall)
            (p/'gt710-pixels.rle').write_bytes(data + b'x')
            with self.assertRaises(ValueError): API['capture_serial'](p)

    def test_worst_case_rows_fit_unchanged_bulk_limit(self):
        # No adjacent equal words: the maximum count:word length at this pitch.
        data = b''.join((i+1).to_bytes(4, 'little') for i in range(4096))
        serial, rows = capture(self.report, {key: data for key in self.images})
        self.assertLessEqual(len(rows.encode('ascii')) + 1326749, 2 << 20)
        self.assertIn('GT710_CONTEXTS ', serial)


if __name__ == '__main__':
    unittest.main()
