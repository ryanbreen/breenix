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
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'scripts/metal'))
import metal_score
API = runpy.run_path(str(ROOT / 'scripts/metal/gt710-suite'))
MANIFEST = json.loads((ROOT / 'docs/suites/ui-testing.json').read_text())
NAMES = [c['id'] + '/' + t['id'] for c in MANIFEST['categories'] for t in c['cases']]
SELECTED = json.loads((ROOT / 'docs/gui/gt710-step-22-1.json').read_text())['cases']
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
    return f'SUITE_EVIDENCE ui-testing {label} bytes={len(body)} sha256={hashlib.sha256(body).hexdigest()}\n'


def serial():
    lines = ['[gpu] gt710.clocks=full', 'GT710_CLOCK_STARTUP full=true status=ok',
             anchor('startup', BODIES[0]).strip(),
             f'SUITE ui-testing START cases={len(NAMES)}',
             'SUITE ui-testing MEMORY before free=1000 heap_free_kb=500 user_pages=10']
    lines += [f'SUITE ui-testing CASE {n} ' + ('PASS ms=1' if n in SELECTED else 'FAIL ms=1 msg=pending')
              for n in NAMES]
    lines += [f'SUITE ui-testing DONE passed={len(SELECTED)} failed={len(NAMES) - len(SELECTED)} skipped=0 total={len(NAMES)}',
              'SUITE ui-testing MEMORY after free=1000 heap_free_kb=500 user_pages=10',
              'SUITE ui-testing MEMORY reclaimed=PASS free_before=1000 free_after=1000 allowance=48',
              anchor('suite-final-panel', BODIES[1]).strip(),
              'SUITE ui-testing ELAPSED ms=1000', 'SUITE_SEQUENCE READY ui-testing']
    return '\n'.join(lines) + '\n'


class UiStep(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.path = Path(self.tmp.name)
        self.boot = dict(id=BOOT, commit=SHA, toolsCommit=SHA, kind='boot', mode='suite',
                         suite='ui-testing', platform='metal', arch='x86_64', evidenceEligible=True,
                         build=dict(commit=SHA, profile='production', features=[]))
        for name, value in {'run.json': json.dumps(self.boot), 'serial.txt': serial(),
                            'events.jsonl': '\n'.join(json.dumps(e) for e in [
                                dict(event='final-marker'), dict(event='power-off', off=True),
                                dict(event='closed', status=1, endedBy='driver')]) + '\n',
                            'ended': '1', 'ended-by': 'driver', 'exit-status': '1',
                            'dirty': '0', 'commit': SHA}.items():
            (self.path / name).write_text(value)
        self.definition = self.path / 'definition'
        for name in ('scripts/metal/gt710-suite', 'scripts/suite-verdict.py', 'libs/libbreenix/src/suite_memory.rs', 'docs/suites/ui-testing.json',
                     'docs/boot-path.json', 'docs/metal/boot-path.json',
                     'tools/breenix-runs/Sources/BreenixRuns/Resources/boot-stages-x86_64.json'):
            target = self.definition / name
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(ROOT / name, target)
        for n in range(1, 8):
            target = self.definition / f'docs/gui/gt710-step-22-{n}.json'
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(ROOT / f'docs/gui/gt710-step-22-{n}.json', target)
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
        return API['record_ui_capture'](self.path, SHA, 1)

    def test_retained_bundle_record_is_scored_and_bound(self):
        case = 'diagnosis/artifacts-on-mismatch'
        label = 'uitest-' + hashlib.sha256(case.encode()).hexdigest()[:16]
        body = b'UTART001 fixture'
        digest = hashlib.sha256(body).hexdigest()
        record = (f'SUITE ui-testing EVIDENCE bundle {case} label={label} '
                  f'bytes={len(body)} sha256={digest} result=ok path=/var/uitest/ui-testing/{case}\n')
        text = serial().replace(f'SUITE ui-testing CASE {case} ', record + f'SUITE ui-testing CASE {case} ')
        (self.path / f'diagnostic-{label}.bin').write_bytes(body)
        receipt = dict(id=BOOT, route='/gt710-upload/' + 'b' * 64 + '/diagnostics/' + label,
                       complete=True, bytes=len(body), sha256=digest)
        (self.path / f'native-diagnostic-{label}.json').write_text(json.dumps(receipt))
        def score(value):
            (self.path / 'serial.txt').write_text(value)
            return self.record()
        self.assertEqual(score(text), 0)
        for bad in [record.replace(label, 'uitest-' + '0' * 16),
                    record.replace('result=ok', 'result=disk-only:deadline'),
                    record.replace(f'bytes={len(body)}', 'bytes=2097153'),
                    record.replace('path=/var/uitest/ui-testing/', 'path=/wrong/'),
                    record + record]:
            with self.subTest(record=bad):
                self.assertEqual(score(text.replace(record, bad)), 1)
        (self.path / f'diagnostic-{label}.bin').write_bytes(b'corrupt')
        self.assertEqual(score(text), 1)

    def test_selection_and_required_refusals(self):
        self.assertEqual(self.record(), 0)
        self.assertTrue(metal_score.score(self.path)['milestones']['metal-gt710/ui-step']['passed'])
        original = serial()
        mutations = [
            original.replace(f'CASE {SELECTED[0]} PASS ms=1', f'CASE {SELECTED[0]} FAIL ms=1 msg=bad').replace(
                f'DONE passed={len(SELECTED)} failed={len(NAMES) - len(SELECTED)}',
                f'DONE passed={len(SELECTED)-1} failed={len(NAMES) - len(SELECTED)+1}'),
            original.replace(f'CASE {SELECTED[0]} PASS', f'CASE {SELECTED[0]} SKIP'),
            original.replace(f'SUITE ui-testing DONE passed={len(SELECTED)} failed={len(NAMES) - len(SELECTED)} skipped=0 total={len(NAMES)}\n', ''),
            original.replace('clocks=full', 'clocks=post'),
            original.replace(f'total={len(NAMES)}', f'total={len(NAMES)-1}'),
        ]
        for text in mutations:
            with self.subTest(text=text[-300:]):
                (self.path / 'serial.txt').write_text(text)
                self.assertEqual(self.record(), 1)
        (self.path / 'serial.txt').write_text(original)
        (self.path / 'native-final-upload.json').unlink()
        self.assertEqual(self.record(), 1)

    def test_collection_uses_shared_strict_result_contract(self):
        # The startup producer declares local failure collection in the manifest.
        case = 'sessions/start-desktop'
        record = f'SUITE ui-testing COLLECT {case} trigger=fail screen=at-collection windows=1 remnants=0 artifacts=complete\n'
        original = serial()
        at = f'SUITE ui-testing CASE {case} '
        valid = original.replace(at, record + at)
        (self.path / 'serial.txt').write_text(valid)
        self.assertEqual(self.record(), 0)
        for bad in [record.replace('windows=1', 'windows=-1'),
                    record.replace(case, NAMES[1]), record + record,
                    record.replace('COLLECT', 'UNKNOWN'),
                    record.replace('artifacts=complete', 'artifacts=maybe')]:
            with self.subTest(record=bad):
                (self.path / 'serial.txt').write_text(original.replace(at, bad + at))
                self.assertEqual(self.record(), 1)
        (self.path / 'serial.txt').write_text(original.replace(at, '') + record)
        self.assertEqual(self.record(), 1)

    def test_step_four_requires_each_forced_overlap_witness(self):
        selected = json.loads((ROOT / 'docs/gui/gt710-step-22-4.json').read_text())['cases']
        text = serial()
        for name in selected:
            text = text.replace(f'CASE {name} FAIL ms=1 msg=pending', f'CASE {name} PASS ms=1')
        text = text.replace(f'DONE passed={len(SELECTED)} failed={len(NAMES)-len(SELECTED)}',
                            f'DONE passed={len(selected)} failed={len(NAMES)-len(selected)}')
        phases = [f'UI_STARTUP ui-testing sessions/start-{mode} connect-begin=100 first-composite=2000000100\n'
                  for mode in ('desktop', 'kiosk')]
        for mode, phase in zip(('desktop', 'kiosk'), phases):
            text = text.replace(f'SUITE ui-testing CASE sessions/start-{mode} ', phase + f'SUITE ui-testing CASE sessions/start-{mode} ')
        witnesses = []
        for name, rounds in [('capture-while-drawing', 1), ('capture-resize-race', 1)]:
            case = 'frames/' + name
            values = ''.join(f'SUITE ui-testing VALUE {case} {label}={rounds}count\n'
                             for label in ['capture-stall-fired', 'capture-release-fired'])
            text = text.replace(f'SUITE ui-testing CASE {case} ', values + f'SUITE ui-testing CASE {case} ')
            witnesses.append(values)
        def score(body):
            (self.path / 'serial.txt').write_text(body)
            return API['record_ui_capture'](self.path, SHA, 4)
        self.assertEqual(score(text), 0)
        for phase in phases:
            for bad in [text.replace(phase, ''), text.replace(phase, phase + phase),
                        text.replace(phase, phase.replace('first-composite=2000000100', 'first-composite=2000000101'))]:
                self.assertEqual(score(bad), 1)
        for values in witnesses:
            line = values.splitlines(keepends=True)[0]
            for bad in [text.replace(line, '', 1), text.replace(line, line.replace(line.split('=')[1].strip(), '0count'), 1),
                        text.replace(line, line + line, 1), text.replace(line, '', 1) + line]:
                self.assertEqual(score(bad), 1)

    def test_launch_shape_completion_marker(self):
        original = serial()
        path = self.path / 'serial.txt'
        path.write_text(original + 'DISPATCH_STRAND_CENSUS save_no_t')
        self.assertEqual(self.record(), 0)
        path.write_text(original.replace('SUITE_SEQUENCE READY ui-testing\n', ''))
        self.assertEqual(self.record(), 1)
        self.boot['suites'] = ['ui-testing', 'console']
        self.boot['suite'] = None
        (self.path / 'run.json').write_text(json.dumps(self.boot))
        path.write_text(original + 'DISPATCH_STRAND_CENSUS save_no_t')
        with patch.dict(API['step_capture'].__globals__, {'completed_suite_capture': lambda *args: {'boot': self.boot, 'serial': path.read_text()}}):
            with self.assertRaisesRegex(ValueError, 'incomplete or fatal WM capture'):
                API['step_capture'](self.path, SHA, 1, 'ui-testing', 'gt710-step-22-', 7)
        self.boot['suites'] = ['ui-testing']
        self.boot['suite'] = 'ui-testing'
        (self.path / 'run.json').write_text(json.dumps(self.boot))
        path.write_text(original + 'SUITE ui-testing CASE')
        self.assertEqual(self.record(), 1)

    def test_step_five_requires_bounded_case_bound_measurements(self):
        selected = json.loads((ROOT / 'docs/gui/gt710-step-22-5.json').read_text())['cases']
        text = serial()
        for name in selected:
            text = text.replace(f'CASE {name} FAIL ms=1 msg=pending', f'CASE {name} PASS ms=1')
        text = text.replace(f'DONE passed={len(SELECTED)} failed={len(NAMES)-len(SELECTED)}',
                            f'DONE passed={len(selected)} failed={len(NAMES)-len(selected)}')
        for mode in ('desktop', 'kiosk'):
            phase = f'UI_STARTUP ui-testing sessions/start-{mode} connect-begin=100 first-composite=200\n'
            text = text.replace(f'SUITE ui-testing CASE sessions/start-{mode} ', phase + f'SUITE ui-testing CASE sessions/start-{mode} ')
        for name in ('capture-while-drawing', 'capture-resize-race'):
            case = 'frames/' + name
            values = ''.join(f'SUITE ui-testing VALUE {case} {label}=1count\n'
                             for label in ('capture-stall-fired', 'capture-release-fired'))
            text = text.replace(f'SUITE ui-testing CASE {case} ', values + f'SUITE ui-testing CASE {case} ')
        readings = []
        for case, metric, cap in [('sessions/start-desktop', 'startup', 2000000000),
                                  ('sessions/start-kiosk', 'startup', 2000000000),
                                  ('frames/compare-cost', 'compare-ns', 500000000)]:
            line = f'SUITE ui-testing VALUE {case} {metric}=123ns expect=0..{cap}ns\n'
            text = text.replace(f'SUITE ui-testing CASE {case} ', line + f'SUITE ui-testing CASE {case} ')
            readings.append((line, cap))
        def score(body):
            (self.path / 'serial.txt').write_text(body)
            return API['record_ui_capture'](self.path, SHA, 5)
        self.assertEqual(score(text), 0)
        for line, cap in readings:
            for bad in [text.replace(line, ''), text.replace(line, line + line),
                        text.replace(line, line.replace('=123ns', f'={cap+1}ns')),
                        text.replace(line, '', 1) + line]:
                self.assertEqual(score(bad), 1)

    def test_step_seven_is_the_full_manifest_and_scores_as_a_full_suite(self):
        selection = json.loads((ROOT / 'docs/gui/gt710-step-22-7.json').read_text())['cases']
        self.assertEqual(selection, NAMES)
        text = serial()
        for name in NAMES:
            text = text.replace(f'CASE {name} FAIL ms=1 msg=pending', f'CASE {name} PASS ms=1')
        text = text.replace(f'DONE passed={len(SELECTED)} failed={len(NAMES)-len(SELECTED)}',
                            f'DONE passed={len(NAMES)} failed=0')
        log = self.path / 'full-serial.txt'
        log.write_text(text)
        result = subprocess.run([sys.executable, str(ROOT / 'scripts/suite-verdict.py'),
                                 str(ROOT / 'docs/suites/ui-testing.json'), str(log)],
                                capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn('PASS:', result.stdout)

    def test_step_eight_is_refused(self):
        result = subprocess.run([sys.executable, str(ROOT / 'scripts/metal/gt710-suite'),
                                 '--ui-step', '8'], capture_output=True, text=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('invalid choice', result.stderr)

    def test_cli(self):
        result = subprocess.run([sys.executable, str(ROOT / 'scripts/metal/gt710-suite'),
                                 '--ui-step', '1', '--capture-directory', str(self.path), '--sha', SHA],
                                capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

if __name__ == '__main__':
    unittest.main()
