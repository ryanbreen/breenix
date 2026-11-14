"""Drive production supervisor pipe collection/reaping with real host children."""
import importlib.util
import json
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class ExitReporting(unittest.TestCase):
    def test_failed_children_report_bounded_output_but_success_stays_quiet(self):
        runtime = (ROOT / 'libs/libuitest/src/supervisor_runtime.rs').read_text()
        runtime = runtime[runtime.index('struct Pipe('):runtime.index('fn kill_descendants(')]
        wire = (ROOT / 'libs/libuitest/src/supervisor.rs').read_text()
        ring = wire[wire.index('pub const STDERR_CAP:'):wire.index('#[derive(Clone, Debug)]\npub struct SessionEnvironment')]
        fixture = (ROOT / 'tests/fixtures/ui_supervisor_exit_reporting.rs').read_text()
        fixture = fixture.replace("child-stderr-cause", "child-stderr-cause\\rKERNEL PANIC: child-only\\n[bcheck] scrollbar layout FAILED: child-only")
        with tempfile.TemporaryDirectory() as tmp:
            source = Path(tmp) / 'check.rs'
            binary = Path(tmp) / 'check'
            source.write_text(fixture.replace('// PRODUCTION_RUNTIME', ring + runtime))
            built = subprocess.run(['taskpolicy', '-b', 'rustc', '--edition=2021', str(source), '-o', str(binary)], capture_output=True, text=True)
            self.assertEqual(built.returncode, 0, built.stdout + built.stderr)
            self.assertEqual(built.stderr, '', built.stderr)
            ran = subprocess.run([str(binary)], capture_output=True, text=True, timeout=20)
            self.assertEqual(ran.returncode, 0, ran.stdout + ran.stderr)
            self.assertIn('child-stderr-cause', ran.stderr)
            self.assertIn('child-stdout-tail', ran.stderr)
            self.assertIn('SUPERVISOR_FAILURE pid=', ran.stderr)
            self.assertIn('name=/bin/sh status=256', ran.stderr)
            self.assertIn('signal=15', ran.stderr)
            self.assertNotIn('successful-child', ran.stderr)
            self.assertNotIn('discarded-prefix', ran.stderr)
            self.assertLess(len(ran.stderr), 20000)
            child_lines = [line for line in ran.stderr.splitlines() if 'child-only' in line or 'child-stdout-tail' in line]
            self.assertTrue(child_lines)
            for line in child_lines:
                self.assertRegex(line, r'^UI_CHILD pid=[1-9][0-9]* name=/bin/sh \| ')
            spec = importlib.util.spec_from_file_location('scorer', ROOT/'scripts/suite-verdict.py')
            scorer = importlib.util.module_from_spec(spec); spec.loader.exec_module(scorer)
            fixtures = ROOT/'tests/fixtures/suite-verdict'
            manifest = json.loads((fixtures/'manifest.json').read_text())
            serial = Path(tmp)/'serial.txt'
            serial.write_text((fixtures/'all-pass.txt').read_text() + ran.stderr)
            ok, reason = scorer.verdict(manifest, serial, [serial])
            self.assertTrue(ok, reason)
            # Quoting only exempts copied child text; real fatal output still fails.
            for fatal in ['KERNEL PANIC: real fault', '[bcheck] scrollbar layout FAILED: real fault']:
                serial.write_text((fixtures/'all-pass.txt').read_text() + ran.stderr + fatal + '\n')
                ok, reason = scorer.verdict(manifest, serial, [serial])
                self.assertFalse(ok)
                self.assertIn('fatal kernel output', reason)


if __name__ == '__main__':
    unittest.main()
