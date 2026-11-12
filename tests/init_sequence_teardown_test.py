"""Run init's sequence teardown with deterministic process and mount syscalls."""
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

from gt710_gr_live_unwind_test import item

ROOT = Path(__file__).resolve().parents[1]


class SequenceTeardown(unittest.TestCase):
    def test_production_teardown(self):
        relative = 'userspace/programs/src/init_services.rs'
        revision = os.environ.get('INIT_OLD_COMMIT')
        source = (subprocess.check_output(['git', 'show', f'{revision}:{relative}'], text=True)
                  if revision else (ROOT / relative).read_text())
        self.assertIn('pub fn stop_for_sequence(', source,
                      'sequence teardown API absent; poweroff cannot hand off without rebooting')
        methods = '\n'.join(item(source, marker) for marker in (
            '    pub fn stop_for_sequence(', '    fn next_shutdown_service(',
            '    fn sequence_poll(', '    fn reap_children(',
            '    fn enforce_stops(', '    fn stop(', '    fn drain_logs('))
        functions = '\n'.join(item(source, marker) for marker in (
            'fn unmount_sequence_shares(', 'fn sequence_processes(', 'fn exit_text('))
        harness = (ROOT / 'tests/fixtures/init_sequence_teardown.rs').read_text()
        harness = harness.replace('// METHODS', methods).replace('// FUNCTIONS', functions)
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp)
            (path / 'teardown.rs').write_text(harness)
            subprocess.run(['taskpolicy', '-b', 'rustc', '--edition=2021', '-Dwarnings',
                            '--test', str(path / 'teardown.rs'), '-o', str(path / 'teardown')], check=True)
            subprocess.run([str(path / 'teardown'), '--test-threads=1'], check=True)


if __name__ == '__main__':
    unittest.main()
