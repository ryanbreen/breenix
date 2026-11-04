"""Keep a passing BreenGL collection separate from console-panel acceptance."""
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / 'scripts/metal/console-status-acceptance.py'


class ConsoleStatusAcceptance(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        if not SCRIPT.is_file():
            raise AssertionError('console status acceptance must bind both retained captures')
        spec = importlib.util.spec_from_file_location('console_status_acceptance', SCRIPT)
        cls.api = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(cls.api)

    def test_retention_rejects_guest_case_and_dependency_changes(self):
        for name in ('userspace/programs/src/suite_breengel/window_cases/panel.rs',
                     'userspace/programs/src/suite_breengel/window_cases/kernel_restore.rs',
                     'userspace/programs/Cargo.toml', 'docs/gui/breengel.md',
                     'kernel/src/console/status.rs'):
            with self.subTest(name=name), patch('subprocess.run'), patch(
                    'subprocess.check_output', return_value=(name + '\0').encode()):
                with self.assertRaisesRegex(ValueError, 'runtime|contract'):
                    self.api.retained_diff('a' * 40, 'b' * 40)
        names = ['scripts/metal/console-status-acceptance.py',
                 'scripts/metal/gt710-suite', 'tests/console_status_acceptance_test.py',
                 'docs/gui/gt710-step-9-4.json']
        with patch('subprocess.run'), patch('subprocess.check_output',
                return_value=('\0'.join(names) + '\0').encode()):
            self.assertEqual(self.api.retained_diff('a' * 40, 'b' * 40), names)

    def test_console_timing_uses_its_done_byte_offset_with_the_330_second_bound(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp)
            before = b'previous output\n'
            done = b'SUITE console DONE passed=88 failed=0 skipped=0 total=88\n'
            (path / 'serial.txt').write_bytes(before + done + b'SUITE breengel START cases=401\n')
            events = [dict(event='handoff', t=1000),
                      dict(event='final-marker', t=1700)]  # Aggregate completion is later.
            for seconds, valid in [('1330', True), ('1330.001', False), ('1000', False)]:
                (path / 'serial.ts').write_text(f'999\t0\n{seconds}\t{len(before)}\n')
                with self.subTest(seconds=seconds):
                    if valid:
                        self.assertEqual(self.api.console_duration(path, events), 330000)
                    else:
                        with self.assertRaisesRegex(ValueError, '330'):
                            self.api.console_duration(path, events)
            (path / 'serial.ts').write_text('1330\t0\n')
            with self.assertRaisesRegex(ValueError, 'timestamp'):
                self.api.console_duration(path, events)

    def test_passing_suite_cannot_override_stale_identity_or_shared_exit_one(self):
        sha = 'a' * 40
        boot = dict(id='20261104T023723Z-metal', toolsCommit=sha,
                    build=dict(commit=sha, profile='production', features=[]),
                    suite=None, suites=['console', 'breengel'], mode='suite')
        capture = dict(boot=boot, serial='SUITE console DONE passed=88 failed=0 skipped=0 total=88\n'
                       'SUITE console MEMORY reclaimed=PASS free_before=1000 free_after=1000 allowance=48\n',
                       events=[], exit_status=1)
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp)
            (path / 'commit').write_text(sha)
            (path / 'serial.txt').write_text(capture['serial'])
            (path / 'run.json').write_text(json.dumps(boot))
            for stale, status in [(False, 0), (True, 0), (False, 1), (True, 1)]:
                capture['exit_status'] = status
                with self.subTest(stale=stale, status=status), patch.object(
                        self.api, 'retained_diff', return_value=[],
                        side_effect=ValueError('runtime files changed') if stale else None), \
                        patch.dict(self.api.NATIVE,
                            completed_suite_capture=lambda *args: capture,
                            core_console_verdict=lambda *args: None,
                            capture_categories=lambda *args: ['console', 'core'],
                            cases=lambda *args: [dict(id='console/acceptance', result='PASS')]), \
                        patch.object(self.api, 'console_duration', return_value=305505):
                    report = self.api.inspect_capture(path, 'console', 'b' * 40)
                self.assertEqual(report['passed'], not stale and status == 0)
                self.assertEqual(report['commit'], sha)
                self.assertEqual(report['exitStatus'], status)
                self.assertEqual(report['consoleHandoffToDoneMS'], 305505)
                self.assertEqual(report['checks'][0]['result'], 'PASS')
                self.assertEqual(any('runtime' in e for e in report['errors']), stale)
                self.assertEqual(any('exit' in e for e in report['errors']), status == 1)
            self.assertEqual(json.loads((path / 'run.json').read_text()), boot)


if __name__ == '__main__':
    unittest.main()
