"""Run production compose/encoding across destroy-before-admission ENOENT."""
from pathlib import Path
import os
import subprocess
import unittest
from gt710_gr_live_unwind_test import item
from suite_collection_test import run_rust

ROOT = Path(__file__).resolve().parents[1]


class DisappearingWindow(unittest.TestCase):
    def test_close_resnapshots_and_omits_the_removed_sample(self):
        rel = 'userspace/programs/src/bwm/compositor.rs'
        baseline = os.environ.get('BWM_COMPOSE_BASELINE')
        source = subprocess.check_output(['git', 'show', f'{baseline}:{rel}'], cwd=ROOT, text=True) if baseline else (ROOT / rel).read_text()
        fixture = (ROOT / 'tests/fixtures/bwm_disappearing_window.rs').read_text()
        methods = '\n'.join(item(source, marker) for marker in ['pub fn compose(', 'fn encode('])
        methods = methods.replace('super::session::', 'session::')
        fixture = fixture.replace('// COMPOSE_METHODS', methods)
        fixture = fixture.replace('// CONTENT_RECT', item(source, 'fn content_rect(') + '\n' + item(source, 'fn intersect('))
        run_rust(self, fixture)


if __name__ == '__main__':
    unittest.main()
