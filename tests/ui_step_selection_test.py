#!/usr/bin/env python3
"""Verify the cumulative UI testing step selections against the suite manifest."""
import json
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[1]


class UiSelectionsTest(unittest.TestCase):
    def test_cumulative_selections_match_manifest(self):
        manifest = json.loads((ROOT / "docs/suites/ui-testing.json").read_text())
        cases = [f"{category['id']}/{case['id']}"
                 for category in manifest['categories'] for case in category['cases']]
        self.assertEqual(len(set(cases)), len(cases))
        previous = set()
        for step in range(1, 9):
            with self.subTest(step=step):
                selection = json.loads((ROOT / f"docs/gui/gt710-step-22-{step}.json").read_text())
                selected = selection['cases']
                self.assertEqual(selection['suite'], 'ui-testing')
                self.assertEqual(selection['total'], len(selected))
                self.assertEqual(len(set(selected)), len(selected))
                self.assertTrue(previous <= set(selected))
                self.assertEqual(selection['added'], len(set(selected) - previous))
                self.assertEqual(selected, [case for case in cases if case in set(selected)])
                previous = set(selected)
        self.assertEqual(len(cases), 131)
        self.assertEqual(previous, set(cases))


if __name__ == '__main__':
    unittest.main()
