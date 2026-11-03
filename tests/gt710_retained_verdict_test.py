"""Retained native suite evidence must produce a saved, independently checked verdict."""
import json
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
                        API['cases'].__globals__, capture_serial=lambda p: serial):
                    code = API['record_breengel_capture'](directory, 'a' * 40, 'breengel-full')
                record = json.loads((directory / 'gt710-breengel-full.json').read_text())
                self.assertEqual(code, expected)
                self.assertEqual(record['boot'], boot()['id'])
                self.assertEqual(record['commit'], 'a' * 40)
                self.assertEqual(record['milestone'], 'metal-gt710/breengel-full')
                self.assertEqual(record['cases'][0]['result'], 'PASS' if expected == 0 else 'FAIL')

    def test_canonical_verdict_requires_the_full_breengel_record(self):
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp)
            self.capture(directory)
            result = metal_score.score(directory)['milestones'].get('metal-gt710/breengel-full')
            self.assertIsNotNone(result, 'missing GT 710 evidence must fail explicitly')
            self.assertFalse(result['passed'])

    def test_local_replay_preserves_runtime_identity_and_refuses_runtime_changes(self):
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp)
            run = self.capture(directory)
            del run['bootedCommit']  # Authoritative metal records use commit, not the importer alias.
            (directory / 'run.json').write_text(json.dumps(run))
            with patch('subprocess.run'), patch('subprocess.check_output', return_value=b'tests/check.py\0'), \
                    patch.dict(API['cases'].__globals__, capture_serial=lambda p: composition_capture()):
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
            host = dict(API, capture_serial=lambda p: serial)
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
