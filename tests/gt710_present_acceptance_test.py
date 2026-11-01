"""G26 amendment 2: run the core publisher through acceptance and real latch."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'tests'))
from gt710_gr_live_unwind_test import item


class PresentAcceptance(unittest.TestCase):
    def test_acceptance_publishes_wait_without_retiring_the_latch(self):
        display = (ROOT / 'kernel/src/gpu/display.rs').read_text()
        core = (ROOT / 'kernel/src/gpu/mod.rs').read_text()
        fixture = (ROOT / 'tests/fixtures/gt710_present_acceptance.rs').read_text()
        methods = '\n'.join(item(source, marker) for source, marker in [
            (display, 'pub(super) fn present_accepted('),
            (display, 'pub(super) fn presented('),
            (core, '    fn publish('),
            (core, '    fn submit_returned('),
            (core, '    fn retire_if_done('),
        ]).replace('pub(super)', 'pub(crate)')
        # These are the actual return and zero-timeout WAIT predicates, with the
        # scheduler represented by a single poll of the fixture core.
        present = item(display, 'pub(super) fn present_as(')
        answer = present[present.index('    let outcome = wait_for('):present.index('    let outcome = match outcome')]
        wait = item(core, 'fn op_wait(')
        poll = wait[wait.index('    let outcome = wait_for('):wait.index('    match outcome')]
        fixture = fixture.replace('// METHODS', methods).replace('// PRESENT_ANSWER', answer).replace('// WAIT_POLL', poll)
        priority = ['taskpolicy', '-b'] if sys.platform == 'darwin' else []
        with tempfile.TemporaryDirectory() as tmp:
            p = Path(tmp)
            (p / 'accept.rs').write_text(fixture)
            subprocess.run(priority + ['rustc', '--edition=2021', '--test', '-Dwarnings', str(p / 'accept.rs'), '-o', str(p / 'accept')], check=True)
            subprocess.run([str(p / 'accept'), '--nocapture'], check=True)


if __name__ == '__main__':
    unittest.main()
