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
MANIFEST = json.loads((ROOT / 'docs/suites/gui-apps.json').read_text())
NAMES = [c['id'] + '/' + t['id'] for c in MANIFEST['categories'] for t in c['cases']]
SELECTED = json.loads((ROOT / 'docs/gui/gt710-step-23-1.json').read_text())['cases']
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
    return f'SUITE_EVIDENCE gui-apps {label} bytes={len(body)} sha256={hashlib.sha256(body).hexdigest()}\n'


def serial():
    lines = ['[gpu] gt710.clocks=full', 'GT710_CLOCK_STARTUP full=true status=ok',
             anchor('startup', BODIES[0]).strip(),
             f'SUITE gui-apps START cases={len(NAMES)}',
             'SUITE gui-apps MEMORY before free=1000 heap_free_kb=500 user_pages=10']
    lines += [f'SUITE gui-apps CASE {n} ' + ('PASS ms=1' if n in SELECTED else 'FAIL ms=1 msg=pending')
              for n in NAMES]
    lines += [f'SUITE gui-apps DONE passed={len(SELECTED)} failed={len(NAMES) - len(SELECTED)} skipped=0 total={len(NAMES)}',
              'SUITE gui-apps MEMORY after free=1000 heap_free_kb=500 user_pages=10',
              'SUITE gui-apps MEMORY reclaimed=PASS free_before=1000 free_after=1000 allowance=48',
              anchor('suite-final-panel', BODIES[1]).strip(),
              'SUITE gui-apps ELAPSED ms=1000', 'SUITE_SEQUENCE READY gui-apps']
    return '\n'.join(lines) + '\n'


def passing_frame_records(text):
    """Supply the FRAME obligations of the synthetic full-PASS recording."""
    for entry in MANIFEST.get('frameChecks', []):
        case, check = entry['case'], entry['check']
        record = (f'SUITE gui-apps FRAME {case} {check} def=0123abcd ref=formula '
                  f'source=window:app commit=1 epoch=0 w=2 h=1 sha256={"a" * 64} result=match\n')
        marker = f'SUITE gui-apps CASE {case} PASS '
        text = text.replace(marker, record + marker)
    return text


class AppsStep(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.path = Path(self.tmp.name)
        self.boot = dict(id=BOOT, commit=SHA, toolsCommit=SHA, kind='boot', mode='suite',
                         suite='gui-apps', platform='metal', arch='x86_64', evidenceEligible=True,
                         build=dict(commit=SHA, profile='production', features=[]))
        for name, value in {'run.json': json.dumps(self.boot), 'serial.txt': serial(),
                            'events.jsonl': '\n'.join(json.dumps(e) for e in [
                                dict(event='final-marker'), dict(event='power-off', off=True),
                                dict(event='closed', status=1, endedBy='driver')]) + '\n',
                            'ended': '1', 'ended-by': 'driver', 'exit-status': '1',
                            'dirty': '0', 'commit': SHA}.items():
            (self.path / name).write_text(value)
        self.definition = self.path / 'definition'
        for name in ('scripts/metal/gt710-suite', 'scripts/suite-verdict.py', 'libs/libbreenix/src/suite_memory.rs', 'docs/suites/gui-apps.json',
                     'docs/boot-path.json', 'docs/metal/boot-path.json',
                     'tools/breenix-runs/Sources/BreenixRuns/Resources/boot-stages-x86_64.json'):
            target = self.definition / name
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(ROOT / name, target)
        for n in range(1, 2):
            target = self.definition / f'docs/gui/gt710-step-23-{n}.json'
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(ROOT / f'docs/gui/gt710-step-23-{n}.json', target)
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
        return API['record_apps_capture'](self.path, SHA, 1)

    def test_gui_apps_stages_native_evidence_route(self):
        import metal_disk
        class Staged(Exception):
            pass
        with patch.object(metal_disk.metal_persist, 'layout', return_value={'existing': True}), \
             patch.object(metal_disk.metal_persist, 'test_root', return_value=None), \
             patch.object(metal_disk.metal_root, 'console_identity', return_value={}), \
             patch.object(metal_disk.metal_image, 'sha256', return_value='a' * 64), \
             patch.object(metal_disk.m, 'power_token', return_value='test'), \
             patch.object(metal_disk.gt710_upload, 'prepare', return_value='test-route') as prepare, \
             patch.object(metal_disk.metal_root, 'stage_root', side_effect=Staged) as stage:
            with self.assertRaises(Staged):
                metal_disk.stage(self.path, self.path, BOOT, SHA, {},
                                 {'ext2': {'sha256': 'a' * 64}}, mode='suite', suite='gui-apps')
            prepare.assert_called_once_with(self.path, BOOT, {}, suite='gui-apps')
            self.assertEqual(stage.call_args.kwargs['gt710_upload'], 'test-route')

    def test_selection_and_required_refusals(self):
        self.assertEqual(self.record(), 0)
        self.assertTrue(metal_score.score(self.path)['milestones']['metal-gt710/apps-step']['passed'])
        original = serial()
        mutations = [
            original.replace(f'CASE {SELECTED[0]} PASS ms=1', f'CASE {SELECTED[0]} FAIL ms=1 msg=bad').replace(
                f'DONE passed={len(SELECTED)} failed={len(NAMES) - len(SELECTED)}',
                f'DONE passed={len(SELECTED)-1} failed={len(NAMES) - len(SELECTED)+1}'),
            original.replace(f'CASE {SELECTED[0]} PASS', f'CASE {SELECTED[0]} SKIP'),
            original.replace(f'SUITE gui-apps DONE passed={len(SELECTED)} failed={len(NAMES) - len(SELECTED)} skipped=0 total={len(NAMES)}\n', ''),
            original.replace(f'SUITE gui-apps CASE {SELECTED[0]} PASS ms=1\n', ''),
            original.replace(SELECTED[0], 'foreign/case'),
            original.replace(SELECTED[0], '__swap__').replace(SELECTED[1], SELECTED[0]).replace('__swap__', SELECTED[1]),
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

    def test_step_two_selection_and_order(self):
        expected = SELECTED + [
            'blauncher/open', 'blauncher/filter', 'blauncher/no-results',
            'blauncher/keys-select', 'blauncher/launch-gui',
            'blauncher/launch-terminal', 'blauncher/launch-missing', 'blauncher/close',
        ]
        selection_path = ROOT / 'docs/gui/gt710-step-23-2.json'
        selected = json.loads(selection_path.read_text())['cases']
        self.assertEqual(selected, expected)
        self.assertEqual(NAMES[:len(expected)], expected)
        target = self.definition / 'docs/gui/gt710-step-23-2.json'
        shutil.copyfile(selection_path, target)
        with patch.dict(globals(), SELECTED=selected):
            original = serial()
        def record(text):
            (self.path / 'serial.txt').write_text(text)
            return API['record_apps_capture'](self.path, SHA, 2)
        self.assertEqual(record(original), 0)
        first, second = selected[-2:]
        mutations = [
            original.replace(f'SUITE gui-apps CASE {first} PASS ms=1\n', ''),
            original.replace(first, 'foreign/case'),
            original.replace(first, '__swap__').replace(second, first).replace('__swap__', second),
        ]
        for text in mutations:
            with self.subTest(text=text[-400:]):
                self.assertEqual(record(text), 1)

    def test_step_three_selection_and_order(self):
        expected = json.loads((ROOT / 'docs/gui/gt710-step-23-2.json').read_text())['cases'] + [
            'blog/tail', 'blog/follow', 'blog/truncate-rotate', 'blog/navigation',
            'blog/search', 'blog/missing-file', 'blog/blogd-records-once', 'blog/blogd-restart',
        ]
        selection_path = ROOT / 'docs/gui/gt710-step-23-3.json'
        selected = json.loads(selection_path.read_text())['cases']
        self.assertEqual(selected, expected)
        self.assertEqual(NAMES[:len(expected)], expected)
        shutil.copyfile(selection_path, self.definition / 'docs/gui/gt710-step-23-3.json')
        with patch.dict(globals(), SELECTED=selected):
            original = serial()
        def record(text):
            (self.path / 'serial.txt').write_text(text)
            return API['record_apps_capture'](self.path, SHA, 3)
        self.assertEqual(record(original), 0)
        first, second = selected[-2:]
        for text in [original.replace(f'SUITE gui-apps CASE {first} PASS ms=1\n', ''),
                     original.replace(first, 'foreign/case'),
                     original.replace(first, '__swap__').replace(second, first).replace('__swap__', second)]:
            self.assertEqual(record(text), 1)

    def test_step_four_selection_and_order(self):
        expected = json.loads((ROOT / 'docs/gui/gt710-step-23-3.json').read_text())['cases'] + [
            'bfontpicker/lists-fonts', 'bfontpicker/select-apply', 'bfontpicker/persists',
            'bfontpicker/keeps-unknown', 'bfontpicker/live-bterm', 'bfontpicker/save-fails-cleanly',
            'bcheck/results-truthful', 'bcheck/hang', 'bcheck/forged-output',
            'bcheck/totals-exit', 'bcheck/responsive', 'bcheck/production-catalog',
        ]
        selection_path = ROOT / 'docs/gui/gt710-step-23-4.json'
        selected = json.loads(selection_path.read_text())['cases']
        self.assertEqual(selected, expected)
        self.assertEqual(NAMES[:len(expected)], expected)
        shutil.copyfile(selection_path, self.definition / 'docs/gui/gt710-step-23-4.json')
        with patch.dict(globals(), SELECTED=selected):
            original = serial()
        def record(text):
            (self.path / 'serial.txt').write_text(text)
            return API['record_apps_capture'](self.path, SHA, 4)
        # Later, unselected cases fail in this synthetic capture and explain exit 1.
        self.assertEqual(record(original), 0)
        (self.path / 'exit-status').write_text('0')
        events = [dict(event='final-marker'), dict(event='power-off', off=True),
                  dict(event='closed', status=0, endedBy='driver')]
        (self.path / 'events.jsonl').write_text('\n'.join(map(json.dumps, events)) + '\n')
        self.assertEqual(record(original), 0)
        report = json.loads((self.path / 'gt710-apps-step.json').read_text())
        self.assertEqual(report['scope'], 'apps-step-selection')
        self.assertEqual(report['commit'], SHA)
        self.assertEqual(report['toolsCommit'], SHA)
        self.assertEqual(report['build'], self.boot['build'])
        self.assertEqual(report['exitStatus'], 0)
        self.assertEqual(report['selectedCases'],
                         [{'id': name, 'result': 'PASS'} for name in selected])
        self.assertEqual(report['memoryVerdict'],
                         'SUITE gui-apps MEMORY reclaimed=PASS free_before=1000 free_after=1000 allowance=48')
        self.assertEqual(set(report['notMeasured']), {'console-suite', 'gpu-suite'})
        first, second = selected[-2:]
        for text in [original.replace(f'SUITE gui-apps CASE {first} PASS ms=1\n', ''),
                     original.replace(first, 'foreign/case'),
                     original.replace(first, '__swap__').replace(second, first).replace('__swap__', second)]:
            self.assertEqual(record(text), 1)

    def test_step_five_selection_and_order(self):
        expected = json.loads((ROOT / 'docs/gui/gt710-step-23-4.json').read_text())['cases'] + [
            'guskit/pencil', 'guskit/shapes', 'guskit/fill', 'guskit/eraser-color',
            'guskit/undo-redo', 'guskit/save-open', 'guskit/save-fails-cleanly',
            'guskit/smb-save-open', 'guskit/smb-save-fails-cleanly', 'guskit/smb-share-gone',
            'guskit/open-malformed', 'guskit/quit-unsaved', 'guskit/shortcuts',
        ]
        selection_path = ROOT / 'docs/gui/gt710-step-23-5.json'
        selected = json.loads(selection_path.read_text())['cases']
        self.assertEqual(selected, expected)
        self.assertEqual(NAMES[:len(expected)], expected)
        shutil.copyfile(selection_path, self.definition / 'docs/gui/gt710-step-23-5.json')
        with patch.dict(globals(), SELECTED=NAMES):
            original = serial()
        def record(text):
            (self.path / 'serial.txt').write_text(text)
            return API['record_apps_capture'](self.path, SHA, 5)
        # G13 requires a failed case to explain exit 1; this full-PASS capture
        # has no failed case to explain the driver/scorer failure.
        self.assertEqual(record(original), 1)
        (self.path / 'exit-status').write_text('0')
        events = [dict(event='final-marker'), dict(event='power-off', off=True),
                  dict(event='closed', status=0, endedBy='driver')]
        (self.path / 'events.jsonl').write_text('\n'.join(map(json.dumps, events)) + '\n')
        self.assertEqual(record(original), 0)
        report = json.loads((self.path / 'gt710-apps-step.json').read_text())
        self.assertEqual(report['scope'], 'apps-step-selection')
        self.assertEqual(report['commit'], SHA)
        self.assertEqual(report['toolsCommit'], SHA)
        self.assertEqual(report['build'], self.boot['build'])
        self.assertEqual(report['exitStatus'], 0)
        self.assertEqual(report['selectedCases'],
                         [{'id': name, 'result': 'PASS'} for name in selected])
        self.assertEqual(report['memoryVerdict'],
                         'SUITE gui-apps MEMORY reclaimed=PASS free_before=1000 free_after=1000 allowance=48')
        self.assertEqual(set(report['notMeasured']), {'console-suite', 'gpu-suite'})
        first, second = selected[-2:]
        for text in [original.replace(f'SUITE gui-apps CASE {first} PASS ms=1\n', ''),
                     original.replace(first, 'foreign/case'),
                     original.replace(first, '__swap__').replace(second, first).replace('__swap__', second)]:
            self.assertEqual(record(text), 1)

    def test_step_seven_selection_and_order(self):
        expected = json.loads((ROOT / 'docs/gui/gt710-step-23-5.json').read_text())['cases'] + [
            'guskit-collab/join-sync', 'guskit-collab/stroke-both-ways',
            'guskit-collab/global-order', 'guskit-collab/three-peers',
            'guskit-collab/undo-ownership', 'guskit-collab/peer-leaves',
        ]
        selection_path = ROOT / 'docs/gui/gt710-step-23-7.json'
        selected = json.loads(selection_path.read_text())['cases']
        self.assertEqual(selected, expected)
        self.assertEqual([name for name in NAMES if name in selected], selected)
        shutil.copyfile(selection_path, self.definition / 'docs/gui/gt710-step-23-7.json')
        with patch.dict(globals(), SELECTED=NAMES):
            original = serial()
        def record(text):
            (self.path / 'serial.txt').write_text(text)
            return API['record_apps_capture'](self.path, SHA, 7)
        # G13 requires a failed case to explain exit 1; this full-PASS capture
        # has no failed case to explain the driver/scorer failure.
        self.assertEqual(record(original), 1)
        (self.path / 'exit-status').write_text('0')
        events = [dict(event='final-marker'), dict(event='power-off', off=True),
                  dict(event='closed', status=0, endedBy='driver')]
        (self.path / 'events.jsonl').write_text('\n'.join(map(json.dumps, events)) + '\n')
        self.assertEqual(record(original), 0)
        report = json.loads((self.path / 'gt710-apps-step.json').read_text())
        self.assertEqual(report['scope'], 'apps-step-selection')
        self.assertEqual(report['commit'], SHA)
        self.assertEqual(report['toolsCommit'], SHA)
        self.assertEqual(report['build'], self.boot['build'])
        self.assertEqual(report['exitStatus'], 0)
        self.assertEqual(report['selectedCases'],
                         [{'id': name, 'result': 'PASS'} for name in selected])
        self.assertEqual(report['memoryVerdict'],
                         'SUITE gui-apps MEMORY reclaimed=PASS free_before=1000 free_after=1000 allowance=48')
        self.assertEqual(set(report['notMeasured']), {'console-suite', 'gpu-suite'})
        first, second = selected[-2:]
        for text in [original.replace(f'SUITE gui-apps CASE {first} PASS ms=1\n', ''),
                     original.replace(first, 'foreign/case'),
                     original.replace(first, '__swap__').replace(second, first).replace('__swap__', second)]:
            self.assertEqual(record(text), 1)

    def test_cli(self):
        result = subprocess.run([sys.executable, str(ROOT / 'scripts/metal/gt710-suite'),
                                 '--apps-step', '1', '--capture-directory', str(self.path), '--sha', SHA],
                                capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_retained_replay_checks_runtime_diff_and_names_scorer(self):
        head = 'b' * 40
        original = {name: (self.path / name).read_bytes() for name in
                    ('run.json', 'commit', 'serial.txt', 'events.jsonl')}
        changes = ['scripts/metal/gt710-suite', 'tests/gt710_apps_step_test.py']
        with patch('subprocess.run'), patch('subprocess.check_output',
                return_value=('\0'.join(changes) + '\0').encode()):
            self.assertEqual(API['record_apps_capture'](self.path, SHA, 1, scorer_head=head), 0)
        report = json.loads((self.path / 'gt710-apps-step.json').read_text())
        self.assertEqual(report['commit'], SHA)
        self.assertEqual(report['toolsCommit'], SHA)
        self.assertEqual(report['scorerCommit'], head)
        self.assertEqual(report['hostOnlyDiff'], changes)
        self.assertEqual(report['scorerSHA256'], hashlib.sha256(
            (ROOT / 'scripts/metal/gt710-suite').read_bytes()).hexdigest())
        self.assertEqual(report['scorerSources'], {name: hashlib.sha256(
            (ROOT / name).read_bytes()).hexdigest() for name in API['RETAINED_SCORER_FILES']})
        for name, body in original.items():
            self.assertEqual((self.path / name).read_bytes(), body)
        with patch('subprocess.run'), patch('subprocess.check_output',
                return_value=b'userspace/programs/src/bfontpicker.rs\0'):
            self.assertEqual(API['record_apps_capture'](self.path, SHA, 1, scorer_head=head), 1)
        report = json.loads((self.path / 'gt710-apps-step.json').read_text())
        self.assertEqual(report['cases'][0]['result'], 'FAIL')
        self.assertIn('runtime files', report['cases'][0]['reason'])

    def test_missing_pixels_cannot_retain_a_passing_score(self):
        self.assertEqual(self.record(), 0)
        (self.path / 'gt710-pixels.rle').unlink()
        self.assertEqual(self.record(), 1)
        report = json.loads((self.path / 'gt710-apps-step.json').read_text())
        self.assertEqual(report['cases'][0]['result'], 'FAIL')
        self.assertIn('gt710-pixels.rle', report['cases'][0]['reason'])
        self.assertFalse(metal_score.score(self.path)['milestones']['metal-gt710/apps-step']['passed'])

    def test_vigil_import_needs_the_original_uploads_and_receipts(self):
        # Vigil keeps its presentation wrapper beside the driver's run.json.
        (self.path / 'metal-run.json').write_text(json.dumps(self.boot))
        wrapper = {name: self.boot[name] for name in
                   ('id', 'arch', 'mode', 'suite', 'platform')}
        wrapper.update(checkout='', taskName='metal-' + BOOT)
        (self.path / 'run.json').write_text(json.dumps(wrapper))
        artifacts = ('gt710-pixels.rle', 'native-upload.json',
                     'gt710-final.rle', 'native-final-upload.json')
        retained = {name: (self.path / name).read_bytes() for name in artifacts}
        for name in artifacts:
            (self.path / name).unlink()
        self.assertEqual(self.record(), 1)
        report = json.loads((self.path / 'gt710-apps-step.json').read_text())
        self.assertIn('gt710-pixels.rle', report['cases'][0]['reason'])
        for name, body in retained.items():
            (self.path / name).write_bytes(body)
        self.assertEqual(self.record(), 0)
        receipt = json.loads((self.path / 'native-final-upload.json').read_text())
        receipt['sha256'] = '0' * 64
        (self.path / 'native-final-upload.json').write_text(json.dumps(receipt))
        self.assertEqual(self.record(), 1)
        report = json.loads((self.path / 'gt710-apps-step.json').read_text())
        self.assertIn('digest, receipt or route differs', report['cases'][0]['reason'])

    def test_operator_stop_is_not_completed_acceptance(self):
        (self.path / 'exit-status').write_text('143')
        events = [dict(event='final-marker'), dict(event='power-off', off=True),
                  dict(event='closed', status=143, endedBy='driver')]
        (self.path / 'events.jsonl').write_text('\n'.join(map(json.dumps, events)) + '\n')
        self.assertEqual(self.record(), 1)
        report = json.loads((self.path / 'gt710-apps-step.json').read_text())
        self.assertEqual(report['cases'][0]['result'], 'FAIL')
        self.assertEqual(report['cases'][0]['reason'],
                         'native suite requires a complete clean driver-closed capture')

if __name__ == '__main__':
    unittest.main()
