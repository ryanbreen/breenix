"""Replay production probe evidence without another hardware boot."""
from contextlib import redirect_stdout
from io import StringIO
import json
from pathlib import Path
import runpy
import sys
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
API = runpy.run_path(str(ROOT / 'scripts/metal/gt710-suite'))
APERTURE = runpy.run_path(str(ROOT / 'tests/gt710_aperture_report_test.py'))
LIFETIME = runpy.run_path(str(ROOT / 'tests/gt710_lifetime_report_test.py'))
CORE = runpy.run_path(str(ROOT / 'tests/gt710_core_report_test.py'))
sys.path.insert(0, str(ROOT / 'scripts/metal'))
import metal_score

SHA = 'a' * 40
HEAD = 'b' * 40
CATEGORIES = {'inventory', 'vm', 'aperture', 'lifetime', 'memory'}


class ProbeRescore(unittest.TestCase):
    def capture(self, directory):
        serial, aperture = APERTURE['fixture']()
        _, lifetime = LIFETIME['fixture']()
        serial += ('GT710_APERTURE ' + json.dumps(aperture) + '\n'
                   + 'GT710_LIFETIME ' + json.dumps(lifetime) + '\n'
                   + 'PROBE framebuffer OK GPU scanout drawn and read back\n'
                   + 'PROBE DONE passed=17 failed=0\n')
        boot = dict(id='20261104T060703Z-metal', commit=SHA, toolsCommit=SHA,
                    arch='x86_64', platform='metal', kind='boot', mode='probe',
                    suite=None, suites=None, disk='root', evidenceEligible=True, watch=False,
                    build=dict(commit=SHA, profile='production', features=[]))
        files = {'run.json': json.dumps(boot), 'commit': SHA, 'dirty': '0',
                 'ended': '1', 'ended-by': 'driver', 'exit-status': '0', 'serial.txt': serial}
        events = [dict(event='handoff', seconds=10), dict(event='final-marker', seconds=310),
                  dict(event='power-off', off=True), dict(event='closed', status=0, endedBy='driver')]
        files['events.jsonl'] = ''.join(json.dumps(e) + '\n' for e in events)
        for name, value in files.items():
            (directory / name).write_text(value)
        return boot, serial

    def replay(self, directory):
        # Only git ancestry and the authenticated transport are substituted;
        # the category scorers run on independently generated record fixtures.
        serial = (directory / 'serial.txt').read_text()
        with patch('subprocess.run'), patch('subprocess.check_output', return_value=b'tests/check.py\0'), \
                patch.dict(API['rescore_suite_capture'].__globals__, capture_serial=lambda *a: serial), \
                redirect_stdout(StringIO()):
            return API['rescore_suite_capture'](directory, SHA, HEAD)

    def import_capture(self, directory, boot):
        # Vigil preserves the driver's manifest beside its presentation record.
        (directory / 'metal-run.json').write_text(json.dumps(boot))
        wrapper = {k: boot[k] for k in ('id', 'arch', 'platform', 'mode', 'suite', 'disk')}
        wrapper.update(checkout='', taskName='metal-' + boot['id'], watch=False)
        (directory / 'run.json').write_text(json.dumps(wrapper))
        return wrapper

    def test_imported_probe_replay_keeps_and_hashes_both_manifests(self):
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp)
            boot, _ = self.capture(directory)
            self.import_capture(directory, boot)
            originals = {name: (directory / name).read_bytes()
                         for name in ('run.json', 'metal-run.json')}
            self.assertEqual(self.replay(directory), 0)
            report = json.loads((directory / 'suite-acceptance.json').read_text())
            self.assertTrue(report['passed'])
            self.assertEqual(report['toolsCommit'], SHA)
            self.assertEqual(report['build'], boot['build'])
            import hashlib
            for name, data in originals.items():
                self.assertEqual((directory / name).read_bytes(), data)
                self.assertEqual(report['artifacts'][name], hashlib.sha256(data).hexdigest())

    def test_imported_console_uses_driver_identity_and_existing_deadline(self):
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp)
            boot, _ = self.capture(directory)
            boot.update(mode='suite', suite='console', suites=['console'])
            self.import_capture(directory, boot)
            (directory / 'serial.txt').write_text(CORE['console_serial']())
            capture = API['completed_suite_capture'](directory, SHA)
            self.assertEqual(capture['boot'], dict(boot, bootedCommit=SHA))
            self.assertEqual(API['resources_console'](capture['serial'], capture['boot'], capture['events']), 300000)
            events = [dict(e, seconds=340.001) if e['event'] == 'final-marker' else e
                      for e in capture['events']]
            with self.assertRaisesRegex(ValueError, '330 s'):
                API['resources_console'](capture['serial'], capture['boot'], events)

    def test_single_capture_replay_refuses_ignored_step_and_companion_options(self):
        options = [('breengel-fronts',), ('breengel-composition',), ('breengel-stage',),
                   ('breengel-full',), ('resources',), ('alpha',), ('shapes',),
                   ('isolation',), ('presentation',), ('performance',),
                   ('gpu-boot', '20261104T061608Z-metal'),
                   ('core-boot', '20261104T061608Z-metal'),
                   ('acceptance-directory', '/unused'), ('boot', '20261104T061608Z-metal'),
                   ('check-manifest', '/unused'), ('pr', '1')]
        for option in options:
            argv = ['gt710-suite', '--capture-directory', '/unused', '--sha', SHA,
                    '--' + option[0], *option[1:]]
            with self.subTest(option=option), patch.object(sys, 'argv', argv), \
                    patch('subprocess.check_output', return_value=''), \
                    patch.dict(API['main'].__globals__, rescore_suite_capture=lambda *a: 0), \
                    redirect_stdout(StringIO()), patch('sys.stderr', new_callable=StringIO):
                with self.assertRaises(SystemExit) as error:
                    API['main']()
                self.assertEqual(error.exception.code, 2)

    def test_console_replay_names_its_scope_and_missing_gpu_suite(self):
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp)
            boot, _ = self.capture(directory)
            boot.update(mode='suite', suite='console', suites=['console'])
            (directory / 'run.json').write_text(json.dumps(boot))
            capture = dict(boot=boot, serial='fixture', events=[], exit_status=0)
            with patch('subprocess.run'), patch('subprocess.check_output', return_value=b''), \
                    patch.dict(API['rescore_suite_capture'].__globals__,
                               completed_suite_capture=lambda *a: capture,
                               resources_console=lambda *a: 300000,
                               capture_categories=lambda *a: []), redirect_stdout(StringIO()):
                self.assertEqual(API['rescore_suite_capture'](directory, SHA, HEAD), 0)
            report = json.loads((directory / 'suite-acceptance.json').read_text())
            self.assertEqual(report['scope'], 'console-suite-and-startup')
            self.assertIn('gpu-suite', report['notMeasured'])

    def watch_replay(self, directory, gpu=None, failed_suite=None):
        def score(path, sha, head):
            suite = 'console' if path == directory else 'gpu'
            report = dict(boot='20261104T060703Z-metal' if suite == 'console' else '20261104T061608Z-metal',
                          commit=sha, toolsCommit=sha, scorerCommit=head,
                          build=dict(commit=sha, profile='production', features=[]),
                          scope=suite + '-suite-and-startup', passed=suite != failed_suite,
                          checks=[dict(id='graphics/channel', result='PASS', reason='')])
            report['consoleHandoffToDoneMS' if suite == 'console' else 'gpuElapsedMS'] = 300000
            (path / 'suite-acceptance.json').write_text(json.dumps(report))
            return int(not report['passed'])
        with patch.dict(API['rescore_watch_acceptance'].__globals__, rescore_suite_capture=score), \
                redirect_stdout(StringIO()):
            return API['rescore_watch_acceptance'](directory, gpu, SHA, HEAD)

    def test_watch_acceptance_cannot_pass_with_only_console_startup_checks(self):
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp)
            self.assertEqual(self.watch_replay(directory), 1)
            proof = json.loads((directory / 'watch-acceptance.json').read_text())
            self.assertFalse(proof['passed'])
            self.assertTrue(proof['captures']['console']['passed'])
            self.assertIn('GPU-suite capture', proof['errors']['gpu'])

    def test_watch_acceptance_requires_both_suite_verdicts_and_preserves_identities(self):
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp)
            gpu = directory / 'gpu'
            gpu.mkdir()
            for failed in (None, 'console', 'gpu'):
                with self.subTest(failed=failed):
                    self.assertEqual(self.watch_replay(directory, gpu, failed), int(failed is not None))
                    proof = json.loads((directory / 'watch-acceptance.json').read_text())
                    self.assertEqual(proof['passed'], failed is None)
                    self.assertEqual(set(proof['captures']), {'console', 'gpu'})
                    for capture in proof['captures'].values():
                        self.assertEqual(capture['commit'], SHA)
                        self.assertEqual(capture['toolsCommit'], SHA)
                        self.assertEqual(capture['build']['commit'], SHA)
                        self.assertEqual(capture['scorerCommit'], HEAD)

    def test_watch_acceptance_rejects_a_companion_from_another_runtime(self):
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp)
            gpu = directory / 'gpu'
            gpu.mkdir()
            boot, _ = self.capture(gpu)
            boot.update(mode='suite', suite='gpu', suites=['gpu'], commit=HEAD, toolsCommit=HEAD,
                        build=dict(commit=HEAD, profile='production', features=[]))
            (gpu / 'run.json').write_text(json.dumps(boot))
            # Use the real capture identity check; only the unrelated console
            # leg and git transport are substituted.
            rescore = API['rescore_suite_capture']
            def score(path, sha, head):
                if path == directory:
                    self.watch_replay(directory)
                    return 0
                return rescore(path, sha, head)
            with patch('subprocess.run'), patch('subprocess.check_output', return_value=b''), \
                    patch.dict(rescore.__globals__, rescore_suite_capture=score), redirect_stdout(StringIO()):
                self.assertEqual(API['rescore_watch_acceptance'](directory, gpu, SHA, HEAD), 1)
            proof = json.loads((directory / 'watch-acceptance.json').read_text())
            self.assertFalse(proof['passed'])
            self.assertIn('gpu', proof['errors'])

    def test_imported_identity_rejects_conflicts_and_incomplete_driver_manifest(self):
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp)
            boot, _ = self.capture(directory)
            wrapper = self.import_capture(directory, boot)
            for key, value in [('id', '20261104T061608Z-metal'), ('mode', 'suite'),
                               ('suite', 'gpu'), ('commit', HEAD), ('toolsCommit', HEAD),
                               ('kind', 'fault-test'), ('evidenceEligible', False),
                               ('onlyCase', 'device/caps')]:
                (directory / 'run.json').write_text(json.dumps(dict(wrapper, **{key: value})))
                with self.subTest(field=key), self.assertRaises(ValueError):
                    self.replay(directory)
            (directory / 'run.json').write_text(json.dumps(dict(wrapper, toolsCommit=SHA)))
            incomplete = dict(boot)
            del incomplete['toolsCommit']
            (directory / 'metal-run.json').write_text(json.dumps(incomplete))
            with self.assertRaises(ValueError):
                self.replay(directory)

    def test_probe_replay_scores_existing_diagnostics_and_preserves_identity(self):
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp)
            boot, _ = self.capture(directory)
            self.assertEqual(self.replay(directory), 0)
            report = json.loads((directory / 'suite-acceptance.json').read_text())
            self.assertTrue(report['passed'])
            self.assertEqual(report['commit'], SHA)
            self.assertEqual(report['toolsCommit'], SHA)
            self.assertEqual(report['scorerCommit'], HEAD)
            self.assertEqual(report['probeHandoffToDoneMS'], 300000)
            self.assertEqual({r['id'].split('/')[0] for r in report['checks']}, CATEGORIES)
            self.assertEqual(json.loads((directory / 'run.json').read_text()), boot)
            for category in CATEGORIES:
                record = json.loads((directory / ('gt710-' + category + '.json')).read_text())
                self.assertEqual(record['commit'], SHA)
                self.assertEqual(record['cases'][0]['result'], 'PASS')
            # Corrupt protected memory fails even with a valid DONE marker.
            serial = (directory / 'serial.txt').read_text()
            serial = serial.replace('"protected_after": "' + 'a' * 64, '"protected_after": "' + 'b' * 64)
            (directory / 'serial.txt').write_text(serial)
            self.assertEqual(self.replay(directory), 1)
            report = json.loads((directory / 'suite-acceptance.json').read_text())
            self.assertFalse(report['passed'])
            self.assertTrue(any(r['result'] == 'FAIL' for r in report['checks']))

    def test_probe_replay_rejects_deadline_identity_and_incomplete_lifecycle(self):
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp)
            boot, serial = self.capture(directory)
            changes = [('events.jsonl', (directory / 'events.jsonl').read_text().replace('310', '310.001')),
                       ('events.jsonl', '{"event":"closed","status":0,"endedBy":"driver"}\n'),
                       ('exit-status', '1'), ('dirty', '1'),
                       ('serial.txt', serial.replace('PROBE framebuffer OK', 'PROBE framebuffer FAIL')),
                       ('serial.txt', serial.replace('PROBE DONE passed=17 failed=0', '')),
                       ('run.json', json.dumps(dict(boot, build=dict(boot['build'], commit=HEAD)))),
                       ('run.json', json.dumps(dict(boot, toolsCommit=HEAD)))]
            for name, value in changes:
                path = directory / name
                original = path.read_text()
                path.write_text(value)
                with self.subTest(file=name), self.assertRaises(ValueError):
                    self.replay(directory)
                path.write_text(original)

    def test_canonical_probe_verdict_reports_missing_diagnostics(self):
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp)
            self.capture(directory)
            for name, value in [('docs/metal/boot-path.json', dict(milestones=[])),
                                ('tools/breenix-runs/Sources/BreenixRuns/Resources/boot-stages-x86_64.json', dict(stages=[])),
                                ('docs/boot-path.json', dict(fatal=[]))]:
                path = directory / 'definition' / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text(json.dumps(value))
            with patch.object(metal_score.runpy, 'run_path', return_value=API):
                verdict = metal_score.score(directory)
            for category in CATEGORIES:
                row = verdict['milestones'].get('metal-gt710/' + category)
                self.assertIsNotNone(row, 'a missing diagnostic must fail explicitly')
                self.assertFalse(row['passed'])
                self.assertIn('missing gt710-' + category + '.json', row['reason'])


if __name__ == '__main__':
    unittest.main()
