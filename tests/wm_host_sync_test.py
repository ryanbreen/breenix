"""W14: run the production parking and text cases with a virtual host clock.

Only transport, client storage, scanout and the clock are fixtures. The case
bodies, deadline loop and ACK reader run from the production sources.
"""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

from gt710_gr_live_unwind_test import item

ROOT = Path(__file__).resolve().parents[1]


class HostSync(unittest.TestCase):
    def test_delayed_park_and_key_resend(self):
        cases = (ROOT / 'userspace/programs/src/wm_suite/cases.rs').read_text()
        desktop = (ROOT / 'userspace/programs/src/wm_suite/desktop.rs').read_text()
        suite = (ROOT / 'libs/libbreenix/src/suite.rs').read_text()
        fixture = (ROOT / 'tests/fixtures/wm_host_sync.rs').read_text()
        helpers = '\n'.join(item(suite, marker) for marker in [
            'pub fn hostkeys_acked(', 'pub fn hostpointer_acked(',
            'fn wait_host_ack(', 'fn host_ack_time(',
        ] if marker in suite)
        deadline = item(desktop, 'pub fn until(')
        if 'pub fn until_from(' in desktop:
            deadline += '\n' + item(desktop, 'pub fn until_from(')
        program = fixture.replace('// HELPERS', helpers).replace('// DEADLINE', deadline)
        program = program.replace('// PARKED', item(cases, 'fn parked('))
        text_case = item(cases, 'pub fn text_exact(')
        program = program.replace('// TEXT_EXACT', text_case)
        if 'hostkeys_acked(' in text_case:
            program = program.replace('use breenix_suite::suite::hostkeys;\n', '')
        with tempfile.TemporaryDirectory(prefix='wm-host-sync-') as directory:
            temp = Path(directory)
            # Only redirect the proc file boundary. Parsing and polling stay real.
            program = program.replace('/proc/kmsg', str(temp / 'kmsg'))
            (temp / 'check.rs').write_text(program)
            compiler = (['taskpolicy', '-b'] if sys.platform == 'darwin' else []) + ['rustc', '--edition=2021', '-Dwarnings']
            subprocess.run(compiler + ['--test', str(temp / 'check.rs'), '-o', str(temp / 'check')], check=True)
            subprocess.run([str(temp / 'check'), '--test-threads=1', '--nocapture'], check=True)


if __name__ == '__main__':
    unittest.main()
