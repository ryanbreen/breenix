"""Retained native suite evidence must produce a saved, independently checked verdict."""
import json
import hashlib
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

from gt710_breengel_selection_test import API, ROOT, boot
from gt710_breengel_evidence_test import composition_capture

sys.path.insert(0, str(ROOT / 'scripts/metal'))
import metal_score


class RetainedVerdict(unittest.TestCase):
    def capture(self, directory):
        run = boot()
        run.update(commit='a' * 40, disk='root')
        def write(name, value):
            path = directory / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(json.dumps(value))
        write('run.json', run)
        write('definition/docs/metal/boot-path.json', {'milestones': []})
        write('definition/tools/breenix-runs/Sources/BreenixRuns/Resources/boot-stages-x86_64.json', {'stages': []})
        write('definition/docs/boot-path.json', {'fatal': []})
        write('definition/docs/gui/gt710-step-9-4.json', {})
        for name, value in [('commit', 'a' * 40), ('ended', '1'), ('ended-by', 'driver'),
                            ('dirty', '0'), ('exit-status', '0'), ('serial.txt', composition_capture())]:
            (directory / name).write_text(value + ('\n' if not value.endswith('\n') else ''))
        events = [dict(event='final-marker', t=1), dict(event='power-off', off=True),
                  dict(event='closed', status=0, endedBy='driver')]
        (directory / 'events.jsonl').write_text(''.join(json.dumps(e) + '\n' for e in events))
        return run

    def test_unreadable_capture_replaces_stale_pass(self):
        for broken in ('malformed-run', 'missing-run', 'missing-exit-status'):
            with self.subTest(broken=broken), tempfile.TemporaryDirectory() as tmp:
                directory = Path(tmp)
                self.capture(directory)
                report_path = directory / 'gt710-breengel-full.json'
                report_path.write_text(json.dumps(dict(passed=True)))
                if broken == 'malformed-run':
                    (directory / 'run.json').write_text('{')
                else:
                    (directory / ('run.json' if broken == 'missing-run' else 'exit-status')).unlink()
                self.assertEqual(API['record_breengel_capture'](directory, 'a' * 40, 'breengel-full'), 1)
                report = json.loads(report_path.read_text())
                self.assertFalse(report['passed'])
                self.assertFalse(report['captureAccepted'])
                self.assertEqual(report['cases'][0]['result'], 'FAIL')
                self.assertTrue(report['cases'][0]['reason'])

    def test_rejected_aggregate_saves_failure_and_source_boot(self):
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp)
            run = self.capture(directory)
            run.update(suite=None, suites=['console', 'breengel'])
            (directory / 'run.json').write_text(json.dumps(run))
            (directory / 'exit-status').write_text('1\n')
            for name in run['suites']:
                path = directory / 'definition/docs/suites' / (name + '.json')
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text(json.dumps(dict(categories=[dict(id='case', cases=[dict(id='one')])])))
            (directory / 'suite-verdict.txt').write_text(
                'PASS console: complete\n'
                'NOT-ISOLATED breengel: RESET breengel changed=1 paths=/var/log/auth.log\n'
                'FAIL: 1 of 2 suites passed\n')
            original = {name: (directory / name).read_bytes() for name in
                        ('run.json', 'serial.txt', 'events.jsonl', 'exit-status', 'suite-verdict.txt')}
            # A previous successful report must not survive a rejected re-score.
            path = directory / 'gt710-breengel-full.json'
            path.write_text(json.dumps(dict(passed=True)))
            self.assertEqual(API['record_breengel_capture'](directory, 'a' * 40, 'breengel-full'), 1)
            report = json.loads(path.read_text())
            self.assertFalse(report['passed'])
            self.assertFalse(report['captureAccepted'])
            self.assertEqual(report['cases'][0]['result'], 'FAIL')
            self.assertIn('aggregate has an unplanned refusal: NOT-ISOLATED breengel',
                          report['cases'][0]['reason'])
            self.assertEqual(report['boots'][0]['id'], run['id'])
            self.assertEqual(report['boots'][0]['commit'], run['commit'])
            self.assertEqual(report['boots'][0]['toolsCommit'], run['toolsCommit'])
            self.assertEqual(report['boots'][0]['build'], run['build'])
            self.assertEqual(report['boots'][0]['exitStatus'], 1)
            for name, data in original.items():
                self.assertEqual((directory / name).read_bytes(), data)
                self.assertEqual(report['boots'][0]['sha256'][name], hashlib.sha256(data).hexdigest())

    def test_breengel_cli_saves_pass_and_failed_selected_case(self):
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp)
            self.capture(directory)
            original = composition_capture()
            failed = original.replace('CASE composition/present-rate PASS ms=1',
                                      'CASE composition/present-rate FAIL ms=1')
            failed = failed.replace(next(l for l in failed.splitlines() if
                l.startswith('SUITE breengel CASE composition/present-rate ')),
                next(l for l in failed.splitlines() if
                l.startswith('SUITE breengel CASE composition/present-rate ')) + ' msg=59fps')
            failed = failed.replace('DONE passed=401 failed=0', 'DONE passed=400 failed=1')
            for serial, expected in [(original, 0), (failed, 1)]:
                with self.subTest(expected=expected), patch.dict(
                        API['cases'].__globals__, capture_serial=lambda p, *args: serial):
                    code = API['record_breengel_capture'](directory, 'a' * 40, 'breengel-full')
                record = json.loads((directory / 'gt710-breengel-full.json').read_text())
                self.assertEqual(code, expected)
                self.assertEqual(record['boot'], boot()['id'])
                self.assertEqual(record['commit'], 'a' * 40)
                self.assertEqual(record['milestone'], 'metal-gt710/breengel-full')
                self.assertEqual(record['cases'][0]['result'], 'PASS' if expected == 0 else 'FAIL')

    def test_nonzero_breengel_exit_requires_completed_case_failure(self):
        # G13: distinguish a case-derived exit from an unexplained driver exit.
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp)
            self.capture(directory)
            original = composition_capture()
            selected = json.loads((ROOT / 'docs/gui/gt710-step-9-1.json').read_text())['cases']
            manifest = json.loads((ROOT / 'docs/suites/breengel.json').read_text())
            name = next(c['id'] + '/' + case['id'] for c in manifest['categories']
                        for case in c['cases'] if c['id'] + '/' + case['id'] not in selected)
            failed = original.replace(f'CASE {name} PASS ms=1',
                                      f'CASE {name} FAIL ms=1')
            line = next(l for l in failed.splitlines() if l.startswith(f'SUITE breengel CASE {name} '))
            failed = failed.replace(line, line + ' msg=pending')
            failed = failed.replace('DONE passed=401 failed=0', 'DONE passed=400 failed=1')
            events = [dict(event='final-marker'), dict(event='decided', status=0),
                      dict(event='power-off', off=True), dict(event='suite-verdict', status=1),
                      dict(event='closed', status=1, endedBy='driver')]
            (directory / 'exit-status').write_text('1\n')
            def read(serial, trace):
                (directory / 'events.jsonl').write_text(''.join(json.dumps(e) + '\n' for e in trace))
                with patch.dict(API['completed_suite_capture'].__globals__,
                                capture_serial=lambda *a: serial):
                    return API['completed_suite_capture'](directory, 'a' * 40, 'breengel')
            self.assertEqual(read(failed, events)['exit_status'], 1)
            for serial, trace in [(original, events), (failed, events[:3] + events[4:]),
                                  (failed, [events[0], dict(event='decided', status=1)] + events[2:])]:
                with self.subTest(failed_cases=serial == failed, trace=trace):
                    with self.assertRaisesRegex(ValueError, 'BreenGL exit 1'):
                        read(serial, trace)

    def test_canonical_verdict_requires_the_full_breengel_record(self):
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp)
            run = self.capture(directory)
            for suite, suites in [('breengel', ['breengel']), (None, ['console', 'breengel'])]:
                with self.subTest(suites=suites):
                    (directory / 'run.json').write_text(json.dumps(dict(run, suite=suite, suites=suites)))
                    result = metal_score.score(directory)['milestones'].get('metal-gt710/breengel-full')
                    self.assertIsNotNone(result, 'missing GT 710 evidence must fail explicitly')
                    self.assertFalse(result['passed'])

    def test_closed_native_suites_require_startup_records(self):
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp)
            run = self.capture(directory)
            for suite in ('console', 'gpu'):
                run.update(suite=suite, suites=[suite])
                (directory / 'run.json').write_text(json.dumps(run))
                with self.subTest(suite=suite), patch.object(
                        metal_score.runpy, 'run_path', return_value=API):
                    verdict = metal_score.score(directory)
                    expected = [c for c in API['capture_categories'](run) if c != 'core']
                    self.assertTrue(expected)
                    for category in expected:
                        result = verdict['milestones'].get('metal-gt710/' + category)
                        self.assertIsNotNone(result, 'lost startup records must fail explicitly')
                        self.assertFalse(result['passed'])
                        self.assertIn('missing gt710-' + category + '.json', result['reason'])
                    self.assertTrue(metal_score.falls_short(verdict))

    def test_gpu_replay_persists_the_startup_cases_it_judges(self):
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp)
            run = self.capture(directory)
            run.update(suite='gpu', suites=['gpu'])
            (directory / 'run.json').write_text(json.dumps(run))
            capture = dict(boot=run, serial='authenticated fixture', events=[], exit_status=0)
            rows = [dict(id='board/inventory', result='PASS', reason='')]
            with patch('subprocess.run'), patch('subprocess.check_output', return_value=b'tests/check.py\0'), \
                    patch.dict(API['rescore_suite_capture'].__globals__,
                        completed_suite_capture=lambda *a: capture,
                        resources_suite=lambda *a: dict(cases=[dict(id='gpu/fixture', result='PASS')], elapsed_ms=1),
                        capture_categories=lambda *a: ['board', 'core'],
                        cases=lambda *a: rows, score=lambda *a: 'board: passed'):
                self.assertEqual(API['rescore_suite_capture'](directory, 'a' * 40, 'b' * 40), 0)
            record = json.loads((directory / 'gt710-board.json').read_text())
            self.assertEqual(record, dict(boot=run['id'], commit='a' * 40,
                                         milestone='metal-gt710/board', cases=rows))
            self.assertFalse((directory / 'gt710-core.json').exists())
            report = json.loads((directory / 'suite-acceptance.json').read_text())
            self.assertEqual(report['checks'], rows)
            self.assertEqual(report['scorerCommit'], 'b' * 40)
            self.assertEqual(json.loads((directory / 'run.json').read_text()), run)

    def test_driver_finalization_does_not_require_a_post_close_receipt(self):
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp)
            self.capture(directory)
            (directory / 'ended').unlink()
            result = metal_score.score(directory)['milestones'].get('metal-gt710/breengel-full')
            self.assertIsNone(result, 'the driver writes closure after its startup milestone verdict')

    def test_diagnostic_breengel_does_not_require_full_gate_receipt(self):
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp)
            run = self.capture(directory)
            run.update(onlyCase='composition', evidenceEligible=False, watch=True)
            (directory / 'run.json').write_text(json.dumps(run))
            with self.assertRaisesRegex(ValueError, 'filtered suite'):
                metal_score.score(directory)
            run.pop('onlyCase')
            run.update(watch=False, build=dict(run['build'], profile='development'))
            (directory / 'run.json').write_text(json.dumps(run))
            result = metal_score.score(directory)['milestones'].get('metal-gt710/breengel-full')
            self.assertIsNone(result)

    def test_aggregate_closure_requires_successful_cold_readback(self):
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp)
            run = self.capture(directory)
            run.update(suite=None, suites=['console', 'breengel', 'files-io'])
            (directory / 'run.json').write_text(json.dumps(run))
            events = [json.loads(line) for line in (directory / 'events.jsonl').read_text().splitlines()]
            events[-1]['status'] = 1  # The driver awaits its cold disk verdict.
            child = '20261103T120000Z-metal'
            events.extend([dict(event='suite-verdict', status=0),
                           dict(event='cold-disk-verdict', readback=child, passed=True)])
            (directory / 'events.jsonl').write_text(''.join(json.dumps(e) + '\n' for e in events))
            receipt = dict(id=child, bytes=4096, sha256='c' * 64, passed=True)
            with patch.dict(API['completed_suite_capture'].__globals__, capture_serial=lambda p, *args: composition_capture()):
                (directory / 'suite-readback.json').write_text(json.dumps(receipt))
                capture = API['completed_suite_capture'](directory, 'a' * 40, 'breengel')
                self.assertEqual(capture['boot']['suites'], run['suites'])
                self.assertEqual(capture['events'], events)
                for changed in [dict(receipt, passed=False), dict(receipt, id='20261103T120001Z-metal'),
                                dict(receipt, sha256='invalid'), dict(receipt, bytes=0)]:
                    (directory / 'suite-readback.json').write_text(json.dumps(changed))
                    with self.assertRaises(ValueError):
                        API['completed_suite_capture'](directory, 'a' * 40, 'breengel')
                (directory / 'suite-readback.json').unlink()
                with self.assertRaises((ValueError, OSError)):
                    API['completed_suite_capture'](directory, 'a' * 40, 'breengel')

    def test_local_replay_preserves_runtime_identity_and_refuses_runtime_changes(self):
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp)
            run = self.capture(directory)
            del run['bootedCommit']  # Authoritative metal records use commit, not the importer alias.
            (directory / 'run.json').write_text(json.dumps(run))
            with patch('subprocess.run'), patch('subprocess.check_output', return_value=b'tests/check.py\0'), \
                    patch.dict(API['cases'].__globals__, capture_serial=lambda p, *args: composition_capture()):
                self.assertEqual(API['rescore_suite_capture'](directory, 'a' * 40, 'b' * 40), 0)
            report = json.loads((directory / 'suite-acceptance.json').read_text())
            self.assertEqual(report['commit'], 'a' * 40)
            self.assertEqual(report['scorerCommit'], 'b' * 40)
            self.assertTrue(report['passed'])
            self.assertEqual(json.loads((directory / 'run.json').read_text()), run)
            with patch('subprocess.run'), patch('subprocess.check_output', return_value=b'kernel/src/gpu/compose.rs\0'):
                with self.assertRaisesRegex(ValueError, 'runtime files'):
                    API['rescore_suite_capture'](directory, 'a' * 40, 'b' * 40)

    def test_canonical_verdict_recomputes_saved_breengel_result(self):
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp)
            run = self.capture(directory)
            serial = composition_capture()
            records = dict(boot=run['id'], commit='a' * 40, milestone='metal-gt710/breengel-full',
                           cases=API['cases']('breengel-full', '', {'breengel': dict(boot=run, serial=serial)}))
            path = directory / 'gt710-breengel-full.json'
            path.write_text(json.dumps(records))
            host = dict(API, capture_serial=lambda p, *args: serial)
            run_path = metal_score.runpy.run_path
            def load(path):
                return host if str(path).endswith('/scripts/metal/gt710-suite') else run_path(path)
            with patch.object(metal_score.runpy, 'run_path', side_effect=load):
                result = metal_score.score(directory)['milestones']['metal-gt710/breengel-full']
                self.assertTrue(result['passed'], result)
                records['cases'][0]['reason'] = 'altered result'
                path.write_text(json.dumps(records))
                result = metal_score.score(directory)['milestones']['metal-gt710/breengel-full']
                self.assertFalse(result['passed'])


if __name__ == '__main__':
    unittest.main()
