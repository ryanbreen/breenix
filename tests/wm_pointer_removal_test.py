"""Check W27's suite removal and strict, manifest-sized WM scoring."""
import json
import re
import unittest
from unittest.mock import patch

import gt710_wm_step_test as fixture

REMOVED = 'input-devices/pointer-completion-path'


def capture(names, failures=()):
    text = fixture.serial()
    text = re.sub(r'START cases=\d+', f'START cases={len(names)}', text)
    rows = ''.join(f'SUITE window-manager CASE {name} '
                   + ('FAIL ms=1 msg=pending\n' if name in failures else 'PASS ms=1\n')
                   for name in names)
    text = re.sub(r'(?:^SUITE window-manager CASE .*\n)+', lambda _: rows, text, flags=re.M)
    failed = sum(name in failures for name in names)
    return re.sub(r'DONE passed=\d+ failed=\d+ skipped=0 total=\d+',
                  f'DONE passed={len(names) - failed} failed={failed} skipped=0 total={len(names)}', text)


class PointerRemoval(unittest.TestCase):
    def setUp(self):
        self.fixture = fixture.WmStep()
        self.fixture.setUp()
        self.addCleanup(self.fixture.doCleanups)
        self.names = [name for name in fixture.NAMES if name != REMOVED]

    def score(self, names, failures=()):
        (self.fixture.path / 'serial.txt').write_text(capture(names, failures))
        return fixture.API['wm_step_capture'](self.fixture.path, fixture.SHA, 7)

    def test_manifest_runner_and_step_selections_agree_without_removed_case(self):
        self.assertNotIn(REMOVED, fixture.NAMES)
        source = (fixture.ROOT / 'userspace/programs/src/suite_window_manager.rs').read_text()
        category = None
        runner = []
        for kind, name in re.findall(r'\b(category|case)\("([a-z0-9-]+)"', source):
            if kind == 'category':
                category = name
            else:
                runner.append(category + '/' + name)
        self.assertEqual(runner, fixture.NAMES)
        previous = set()
        for number in range(1, 8):
            selection = json.loads((fixture.ROOT / f'docs/gui/gt710-step-21-{number}.json').read_text())
            selected = selection['cases']
            with self.subTest(step=number):
                self.assertNotIn(REMOVED, selected)
                self.assertEqual(selection['total'], len(selected))
                self.assertEqual(selected, [name for name in runner if name in selected])
                self.assertTrue(previous <= set(selected))
                self.assertEqual(selection['added'], len(set(selected) - previous))
            previous = set(selected)
        self.assertEqual(previous, set(runner))

    def test_manifest_capture_passes(self):
        report = self.score(self.names)
        self.assertEqual(report['cases'][0]['result'], 'PASS')
        self.assertNotIn('expectedFailures', report)
        self.assertIn(f'{len(self.names)} cases, memory and uploads verified', report['cases'][0]['reason'])
    def test_stale_removed_records_are_refused(self):
        stale = self.names.copy()
        stale.insert(stale.index('input-devices/replay-report-fault'), REMOVED)
        for failures in ((), (REMOVED,)):
            with self.subTest(failures=failures), self.assertRaisesRegex(ValueError, 'manifest-ordered'):
                self.score(stale, failures)

    def test_selected_failure_is_refused_without_exception(self):
        failed = 'input-devices/keyboard-completion-path'
        with self.assertRaisesRegex(ValueError, 'failed: ' + failed):
            self.score(self.names, (failed,))

    def test_scorer_uses_manifest_size_instead_of_another_literal(self):
        # A smaller definition exercises the same scorer with a different manifest size.
        names = self.names[:-1]
        manifest = json.loads(json.dumps(fixture.MANIFEST))
        for category in manifest['categories']:
            category['cases'] = [case for case in category['cases']
                                 if category['id'] + '/' + case['id'] in names]
        (self.fixture.definition / 'docs/suites/window-manager.json').write_text(json.dumps(manifest))
        selection = dict(suite='window-manager', cases=names, total=len(names))
        (self.fixture.definition / 'docs/gui/gt710-step-21-7.json').write_text(json.dumps(selection))
        scorer = fixture.API['wm_step_capture']
        with patch.dict(scorer.__globals__, __file__=str(self.fixture.definition / 'scripts/metal/gt710-suite')):
            self.assertEqual(self.score(names)['cases'][0]['result'], 'PASS')
            with self.assertRaisesRegex(ValueError, 'manifest-ordered'):
                self.score(self.names)


if __name__ == '__main__':
    unittest.main()
