"""Bind the GPU producer to its own boot alongside retained console/core proof."""
import hashlib
from contextlib import redirect_stdout
from io import StringIO
import json
from pathlib import Path
import runpy
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
CORE = runpy.run_path(str(ROOT / 'tests/gt710_core_report_test.py'))
API = CORE['API']
SHA = CORE['SHA']


def pair(path, producer_sha=SHA):
    console = dict(CORE['gpu_boot'](), suite='console', suites=['console'])
    console['build'] = dict(commit=SHA, profile='production', features=[])
    diagnostic = dict(console, id='20261028T180100Z-metal', mode='probe',
                      suite=None, suites=None, gt710Category='core',
                      build=dict(commit=SHA, profile='gpu-core', features=['gt710_core_probe']))
    for directory, boot in [(path, console), (path / 'core-diagnostic', diagnostic)]:
        directory.mkdir(exist_ok=True)
        files = {'run.json': json.dumps(boot), 'commit': SHA, 'dirty': '0',
                 'ended': '1', 'exit-status': '0',
                 'events.jsonl': '{"event":"handoff","seconds":10}\n'
                                 '{"event":"final-marker","seconds":340}\n'}
        for name, data in files.items():
            (directory / name).write_text(data)
    CORE['stage'](path / 'core-diagnostic')
    startup = b'GT710_CORE_STARTUP {"schema":1,"irq_masked_max_us":37,"ready":true}\n'
    anchor = f'{len(startup)} {hashlib.sha256(startup).hexdigest()}\n'
    manifest = json.loads((ROOT / 'docs/suites/console.json').read_text())
    names = [c['id'] + '/' + case['id'] for c in manifest['categories'] for case in c['cases']]
    serial = ('GT710_CORE_STARTUP_BULK ' + anchor + 'GT710_BULK ' + anchor
              + 'SUITE console START cases=88\n'
              + 'SUITE console MEMORY before free=1000 heap_free_kb=500 user_pages=100\n'
              + ''.join(f'SUITE console CASE {name} PASS ms=1\n' for name in names)
              + 'SUITE console DONE passed=88 failed=0 skipped=0 total=88\n'
              + 'SUITE console MEMORY after free=1000 heap_free_kb=500 user_pages=100\n'
              + 'SUITE console MEMORY reclaimed=PASS free_before=1000 free_after=1000 allowance=48\n'
              + 'SUITE console ELAPSED ms=210\nSUITE_SEQUENCE READY console\n')
    (path / 'serial.txt').write_text(serial)
    (path / 'gt710-pixels.rle').write_bytes(startup)
    gpu = path / 'core-producer'
    gpu.mkdir()
    producer = dict(CORE['gpu_boot'](), id='20261028T180200Z-metal',
                    bootedCommit=producer_sha, toolsCommit=producer_sha,
                    build=dict(commit=producer_sha, profile='production', features=[]))
    for name, data in {'run.json': json.dumps(producer), 'commit': producer_sha,
                       'dirty': '0', 'ended': '1', 'exit-status': '1',
                       'events.jsonl': '{"event":"final-marker","seconds":30}\n',
                       'serial.txt': 'GT710_CORE_STARTUP_BULK ' + anchor + 'GT710_BULK ' + anchor + CORE['gpu_serial']()}.items():
        (gpu / name).write_text(data)
    (gpu / 'gt710-pixels.rle').write_bytes(startup)
    return console


class TwoBootAcceptance(unittest.TestCase):
    def test_producer_requires_gpu_capture_as_well_as_console_and_core(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp)
            boot = pair(path)
            producer = API['core_capture'](path, SHA)
            cases = API['cases']('core', API['capture_serial'](path), producer)
            self.assertEqual([(c['id'], c['result']) for c in cases],
                             [('core/row', 'PASS'), ('core/dispatch', 'PASS'), ('core/producer', 'PASS')])
            report = dict(boot=boot['id'], commit=SHA, milestone='metal-gt710/gpu-core',
                          producerBoot=producer['boot']['id'], cases=cases)
            API['score'](json.loads((ROOT / 'docs/suites/gt710.json').read_text()),
                         json.loads((ROOT / 'docs/efforts/metal.json').read_text()), report,
                         API['capture_serial'](path), boot, SHA, [], producer)
            rows = CORE['records']()
            rows[10]['outcome'] = 'done'  # Masked capability must refuse before publication.
            CORE['stage'](path / 'core-diagnostic', rows)
            failed = API['cases']('core', '', API['core_capture'](path, SHA))
            self.assertEqual(failed[1]['result'], 'FAIL')
            (path / 'core-producer').rename(path / 'missing-producer')
            with self.assertRaisesRegex(ValueError, 'GPU-suite capture'):
                API['core_capture'](path, SHA)

    def test_both_captures_require_the_requested_build_tools_and_clean_exit(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp)
            pair(path)
            API['core_capture'](path, SHA)
            for directory in [path, path / 'core-diagnostic']:
                for name, value in [('exit-status', '1'), ('ended', ''), ('dirty', '1'),
                                    ('commit', 'b' * 40),
                                    ('events.jsonl', '{"event":"console-gap"}\n')]:
                    file = directory / name
                    original = file.read_text()
                    file.write_text(value)
                    with self.subTest(capture=directory.name, file=name), self.assertRaises(ValueError):
                        API['core_capture'](path, SHA)
                    file.write_text(original)
                file = directory / 'run.json'
                original = file.read_text()
                for field in ['toolsCommit', 'bootedCommit', 'build.commit']:
                    changed = json.loads(original)
                    if field == 'build.commit': changed['build']['commit'] = 'b' * 40
                    else: changed[field] = 'b' * 40
                    file.write_text(json.dumps(changed))
                    with self.subTest(capture=directory.name, field=field), self.assertRaises(ValueError):
                        API['core_capture'](path, SHA)
                file.write_text(original)
            events = path / 'events.jsonl'
            events.write_text(events.read_text().replace('340', '340.001'))
            with self.assertRaisesRegex(ValueError, '330'):
                API['core_capture'](path, SHA)

    def test_acceptance_retains_each_boot_binding_and_named_control(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp)
            boot = pair(path)
            milestones = {'metal-gt710/' + ('gpu-core' if c == 'core' else c):
                          dict(passed=True, reached=len(API['CATEGORIES'][c]), total=len(API['CATEGORIES'][c]))
                          for c in API['capture_categories'](boot)}
            with redirect_stdout(StringIO()) as output:
                self.assertTrue(API['acceptance_status'](path, dict(captureComplete=True, milestones=milestones)))
            proof = json.loads((path / 'acceptance.json').read_text())
            self.assertEqual(proof['commit'], SHA)
            self.assertEqual(proof['consoleHandoffToDoneMS'], 330000)
            self.assertEqual(proof['startupIRQMaskedMaxUS'], 37)
            self.assertEqual(proof['coreChecks'], {'core/row': 'PASS', 'core/dispatch': 'PASS',
                                                  'core/producer': 'PASS', 'masked-capability': 'PASS'})
            self.assertEqual(len(proof['boots']), 3)
            self.assertEqual(proof['producerBoot'], '20261028T180200Z-metal')
            self.assertEqual(proof['gpuElapsedMS'], 210)
            for capture in proof['boots']:
                self.assertEqual(capture['commit'], SHA)
                self.assertEqual(capture['toolsCommit'], SHA)
                self.assertEqual(capture['build']['commit'], SHA)
                self.assertEqual(capture['exitStatus'], 1 if capture['id'] == proof['producerBoot'] else 0)
                self.assertIn(capture['id'], output.getvalue())
            self.assertIn('masked-capability PASS', output.getvalue())

    def test_explicit_final_producer_sha_keeps_retained_capture_identities(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp)
            final = 'b' * 40
            boot = pair(path, final)
            with self.assertRaisesRegex(ValueError, 'wrong commit'):
                API['core_capture'](path, SHA)
            producer = API['core_capture'](path, SHA, final)
            cases = API['cases']('core', '', producer)
            report = dict(boot=boot['id'], commit=SHA, milestone='metal-gt710/gpu-core',
                          producerBoot=producer['boot']['id'], cases=cases)
            API['score'](json.loads((ROOT / 'docs/suites/gt710.json').read_text()),
                         json.loads((ROOT / 'docs/efforts/metal.json').read_text()), report,
                         '', boot, SHA, [], producer)
            self.assertEqual(producer['commit'], final)
            self.assertEqual(producer['diagnostic']['boot']['build']['commit'], SHA)
            changed = json.loads((path / 'core-producer/run.json').read_text())
            changed['build']['commit'] = SHA
            (path / 'core-producer/run.json').write_text(json.dumps(changed))
            with self.assertRaisesRegex(ValueError, 'wrong commit'):
                API['core_capture'](path, SHA, final)


if __name__ == '__main__':
    unittest.main()
