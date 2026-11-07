"""Score cumulative WM selections from closed captures and authenticated uploads."""
import hashlib
import json
from pathlib import Path
import runpy
import shutil
import subprocess
import struct
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'scripts/metal'))
import metal_score
API = runpy.run_path(str(ROOT / 'scripts/metal/gt710-suite'))
MANIFEST = json.loads((ROOT / 'docs/suites/window-manager.json').read_text())
NAMES = [c['id'] + '/' + t['id'] for c in MANIFEST['categories'] for t in c['cases']]
SELECTED = json.loads((ROOT / 'docs/gui/gt710-step-21-1.json').read_text())['cases']
SHA = 'a' * 40
BOOT = '20261104T000000Z-metal'
def panel(width=2, height=1, pixels=None):
    record = bytearray(1024)
    for offset, value in [(0, 2), (4, 1024), (16, 6), (20, 5), (24, 1),
                          (40, width), (44, height), (56, width * 4),
                          (128, 0x60000000), (132, 0xa0b50300), (192, 0x60000000)]:
        struct.pack_into('<I', record, offset, value)
    for offset, value in [(80, 41), (120, 73), (184, 97)]:
        struct.pack_into('<Q', record, offset, value)
    if pixels is None:
        pixels = bytes([0, 1, 2, 255, 3, 4, 5, 255])
    return (f'SUITE_SCANOUT v=1 width={width} height={height} encoding=raw record=1024\n'.encode()
            + record + pixels)


BODIES = (panel(), panel(pixels=bytes([5, 4, 3, 255, 2, 1, 0, 255])))


def anchor(label, body):
    return f'SUITE_EVIDENCE window-manager {label} bytes={len(body)} sha256={hashlib.sha256(body).hexdigest()}\n'


def serial():
    lines = ['[gpu] gt710.clocks=full', 'GT710_CLOCK_STARTUP full=true status=ok',
             anchor('startup', BODIES[0]).strip(),
             f'SUITE window-manager START cases={len(NAMES)}',
             'SUITE window-manager MEMORY before free=1000 heap_free_kb=500 user_pages=10']
    lines += [f'SUITE window-manager CASE {n} ' + ('PASS ms=1' if n in SELECTED else 'FAIL ms=1 msg=pending')
              for n in NAMES]
    lines += [f'SUITE window-manager DONE passed={len(SELECTED)} failed={len(NAMES) - len(SELECTED)} skipped=0 total={len(NAMES)}',
              'SUITE window-manager MEMORY after free=1000 heap_free_kb=500 user_pages=10',
              'SUITE window-manager MEMORY reclaimed=PASS free_before=1000 free_after=1000 allowance=48',
              anchor('suite-final-panel', BODIES[1]).strip(),
              'SUITE window-manager ELAPSED ms=1000', 'SUITE_SEQUENCE READY window-manager']
    return '\n'.join(lines) + '\n'


class WmStep(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.path = Path(self.tmp.name)
        self.boot = dict(id=BOOT, commit=SHA, toolsCommit=SHA, kind='boot', mode='suite',
                         suite='window-manager', platform='metal', arch='x86_64', evidenceEligible=True,
                         build=dict(commit=SHA, profile='production', features=[]))
        for name, value in {'run.json': json.dumps(self.boot), 'serial.txt': serial(),
                            'events.jsonl': '\n'.join(json.dumps(e) for e in [
                                dict(event='final-marker'), dict(event='power-off', off=True),
                                dict(event='closed', status=1, endedBy='driver')]) + '\n',
                            'ended': '1', 'ended-by': 'driver', 'exit-status': '1',
                            'dirty': '0', 'commit': SHA}.items():
            (self.path / name).write_text(value)
        self.definition = self.path / 'definition'
        for name in ('scripts/metal/gt710-suite', 'scripts/suite-verdict.py', 'libs/libbreenix/src/suite_memory.rs', 'docs/suites/window-manager.json',
                     'docs/boot-path.json', 'docs/metal/boot-path.json',
                     'tools/breenix-runs/Sources/BreenixRuns/Resources/boot-stages-x86_64.json'):
            target = self.definition / name
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(ROOT / name, target)
        for n in range(1, 8):
            target = self.definition / f'docs/gui/gt710-step-21-{n}.json'
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(ROOT / f'docs/gui/gt710-step-21-{n}.json', target)
        self.uploads(self.path)

    def uploads(self, directory, scope=''):
        directory.mkdir(parents=True, exist_ok=True)
        part_index = 0
        route = '/gt710-upload/' + 'b' * 64 + scope
        def save(filename, receipt, body, path):
            (directory / filename).write_bytes(body)
            (directory / receipt).write_text(json.dumps(dict(
                id=BOOT, route=path, complete=True,
                bytes=len(body), sha256=hashlib.sha256(body).hexdigest())))
        for body, filename, receipt, suffix in (
                (BODIES[0], 'gt710-pixels.rle', 'native-upload.json', ''),
                (BODIES[1], 'gt710-final.rle', 'native-final-upload.json', '/final')):
            if len(body) > 2 << 20:
                chunks = [body[i:i+(2 << 20)] for i in range(0, len(body), 2 << 20)]
                manifest = f'SUITE_PARTS v=1 first={part_index} parts={len(chunks)} bytes={len(body)} sha256={hashlib.sha256(body).hexdigest()}\n'.encode()
                for chunk in chunks:
                    save(f'gt710-breengel-{part_index:04}.rle', f'native-picture-upload-{part_index:04}.json', chunk, route + f'/pictures/{part_index}')
                    part_index += 1
                save(filename, receipt, manifest, route + suffix)
            else:
                save(filename, receipt, body, route + suffix)

    def record(self):
        return API['record_wm_capture'](self.path, SHA, 1)

    def test_closed_failed_suite_can_pass_only_its_selection(self):
        self.assertEqual(self.record(), 0)
        report = json.loads((self.path / 'gt710-wm-step.json').read_text())
        self.assertEqual(report['milestone'], 'metal-gt710/wm-step')
        self.assertEqual(report['selection'], 'gt710-step-21-1.json')
        self.assertTrue(all(row['result'] == 'PASS' for row in report['cases']))
        result = metal_score.score(self.path)['milestones']['metal-gt710/wm-step']
        self.assertTrue(result['passed'], result)

    def test_failed_selection_reports_its_scope_and_case_ids(self):
        step4 = json.loads((ROOT / 'docs/gui/gt710-step-21-4.json').read_text())['cases']
        passed = set(step4) - {'focus/initial-focus'}
        passed.update([name for name in NAMES if name not in step4][:6])
        text = serial()
        for name in NAMES:
            old = f'CASE {name} ' + ('PASS ms=1' if name in SELECTED else 'FAIL ms=1 msg=pending')
            new = f'CASE {name} ' + ('PASS ms=1' if name in passed else 'FAIL ms=1 msg=pending')
            text = text.replace(old, new)
        text = text.replace(f'DONE passed={len(SELECTED)} failed={len(NAMES) - len(SELECTED)}', f'DONE passed={len(passed)} failed={len(NAMES) - len(passed)}')
        (self.path / 'serial.txt').write_text(text)
        for number, required in ((4, step4), (7, NAMES)):
            with self.subTest(step=number):
                self.assertEqual(API['record_wm_capture'](self.path, SHA, number), 1)
                report = json.loads((self.path / 'gt710-wm-step.json').read_text())
                failed = [name for name in required if name not in passed]
                self.assertEqual(report['selection'], f'gt710-step-21-{number}.json')
                self.assertEqual(report['cases'][0]['result'], 'FAIL')
                self.assertEqual(report['cases'][0]['reason'],
                    f'selected WM cases failed: {len(required) - len(failed)}/{len(required)} '
                    f'passed in gt710-step-21-{number}.json; failed: {", ".join(failed)}; '
                    f'full suite: {len(passed)}/{len(NAMES)} passed, {len(NAMES) - len(passed)} failed')

    def test_complete_large_panel(self):
        global BODIES
        original = BODIES
        try:
            pixels = b''.join(struct.pack('<I', 0xff000000 | n) for n in range(600_000))
            BODIES = (panel(1000, 600, pixels), panel(1000, 600, pixels))
            self.uploads(self.path)
            (self.path / 'serial.txt').write_text(serial())
            self.assertEqual(self.record(), 0)
            part = self.path / 'gt710-breengel-0001.rle'
            saved = part.read_bytes()
            part.write_bytes(b'corrupt')
            self.assertEqual(self.record(), 1)
            part.write_bytes(saved)
            # Direct requests remain bounded even with a matching whole digest.
            (self.path / 'gt710-pixels.rle').write_bytes(BODIES[0])
            (self.path / 'native-upload.json').write_text(json.dumps(dict(
                id=BOOT, route='/gt710-upload/'+'b'*64, complete=True,
                bytes=len(BODIES[0]), sha256=hashlib.sha256(BODIES[0]).hexdigest())))
            self.assertEqual(self.record(), 1)
        finally:
            BODIES = original

    def test_cli_scores_a_retained_capture_without_a_boot(self):
        result = subprocess.run([sys.executable, str(ROOT / 'scripts/metal/gt710-suite'),
                                 '--wm-step', '1', '--capture-directory', str(self.path), '--sha', SHA],
                                capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_required_negatives_and_capture_integrity(self):
        original = serial()
        final = anchor('suite-final-panel', BODIES[1])
        mutations = [
            original.replace(f'CASE {SELECTED[0]} PASS ms=1', f'CASE {SELECTED[0]} FAIL ms=1 msg=bad'),
            original.replace('skipped=0', 'skipped=1'),
            original.replace(f'CASE {SELECTED[0]} PASS', f'CASE {SELECTED[0]} SKIP'),
            original.replace(f'SUITE window-manager DONE passed={len(SELECTED)} failed={len(NAMES) - len(SELECTED)} skipped=0 total={len(NAMES)}\n', ''),
            original.replace('clocks=full', 'clocks=post'),
            original.replace('full=true', 'full=false'),
            'GT710_CLOCK_FALLBACK reason=bad\n' + original,
            original.replace(final, ''),
            original.replace(final, '').replace('SUITE window-manager MEMORY reclaimed=', final + 'SUITE window-manager MEMORY reclaimed='),
            original.replace('reclaimed=PASS', 'reclaimed=FAIL'),
            original.replace('allowance=48', 'allowance=64'),
            original.replace(f'total={len(NAMES)}', f'total={len(NAMES) - 1}'),
            original.replace('ELAPSED ms=1000', 'ELAPSED ms=1800001'),
            original.replace(final, final.replace('suite-final-panel', 'case-desktop')),
            original.replace(f'CASE {NAMES[0]}', f'CASE {NAMES[1]}'),
        ]
        for text in mutations:
            with self.subTest(text=text[-450:]):
                (self.path / 'serial.txt').write_text(text)
                self.assertEqual(self.record(), 1)
        (self.path / 'serial.txt').write_text(original)
        for name in ('gt710-pixels.rle', 'gt710-final.rle', 'native-final-upload.json'):
            with self.subTest(file=name):
                saved = (self.path / name).read_bytes()
                (self.path / name).unlink()
                self.assertEqual(self.record(), 1)
                (self.path / name).write_bytes(saved)
        (self.path / 'gt710-final.rle').write_bytes(b'corrupted pixels')
        self.assertEqual(self.record(), 1)

    def test_consistent_memory_shortfall_is_refused(self):
        text = serial().replace('MEMORY after free=1000', 'MEMORY after free=900')
        text = text.replace('reclaimed=PASS free_before=1000 free_after=1000',
                            'reclaimed=FAIL free_before=1000 free_after=900')
        (self.path / 'serial.txt').write_text(text)
        self.assertEqual(self.record(), 1)
        report = json.loads((self.path / 'gt710-wm-step.json').read_text())
        self.assertIn('memory not reclaimed: 100 pages short', report['cases'][0]['reason'])
        self.assertFalse(metal_score.score(self.path)['milestones']['metal-gt710/wm-step']['passed'])

    def test_uploaded_panels_require_plain_scanout_provenance(self):
        global BODIES
        original = BODIES
        try:
            for slot in (0, 1):
                for offset, value in [(24, 3), (24, 0), (132, 0), (128, 0),
                                      (184, 0), (192, 0), (20, 4)]:
                    body = bytearray(original[slot])
                    start = body.index(b'\n') + 1
                    struct.pack_into('<I', body, start + offset, value)
                    BODIES = tuple(bytes(body) if n == slot else original[n] for n in (0, 1))
                    self.uploads(self.path)
                    (self.path / 'serial.txt').write_text(serial())
                    with self.subTest(slot=slot, offset=offset):
                        self.assertEqual(self.record(), 1)
        finally:
            BODIES = original

    def test_missing_or_stale_report_never_scores_green(self):
        missing = metal_score.score(self.path)['milestones'].get('metal-gt710/wm-step')
        self.assertIsNotNone(missing)
        self.assertFalse(missing['passed'])
        self.assertEqual(self.record(), 0)
        (self.path / 'gt710-final.rle').write_bytes(b'changed after scoring')
        self.assertFalse(metal_score.score(self.path)['milestones']['metal-gt710/wm-step']['passed'])

    def test_scoped_sequence_and_cumulative_selection(self):
        self.boot.update(suite=None, suites=['console', 'window-manager'])
        (self.path / 'run.json').write_text(json.dumps(self.boot))
        self.uploads(self.path / 'native-suites/window-manager', '/suites/window-manager')
        (self.path / 'serial.txt').write_text('SUITE console START cases=88\nSUITE_SEQUENCE READY console\n' + serial())
        self.assertEqual(self.record(), 0)
        # Step 2 needs cases outside step 1: its cumulative selection must fail.
        self.assertEqual(API['record_wm_capture'](self.path, SHA, 2), 1)

    def test_startup_uses_the_published_clock_record_and_pre_sample_boundary(self):
        # gt710.clocks names the requested policy; the startup record reports its outcome.
        startup = anchor('startup', BODIES[0])
        text = serial().replace('[gpu] gt710.clocks=full\n', '').replace(startup, '')
        text = text.replace('SUITE window-manager MEMORY before', startup + 'SUITE window-manager MEMORY before')
        (self.path / 'serial.txt').write_text(text)
        self.assertEqual(self.record(), 0)
        text = text.replace(startup, '').replace('SUITE window-manager CASE ', startup + 'SUITE window-manager CASE ', 1)
        (self.path / 'serial.txt').write_text(text)
        self.assertEqual(self.record(), 1)


if __name__ == '__main__':
    unittest.main()
