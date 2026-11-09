#!/usr/bin/env python3
"""Verify the cumulative UI testing step selections against the suite manifest."""
import json
import re
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
        for step in range(1, 8):
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
        self.assertEqual(len(cases), 108)
        self.assertEqual(previous, set(cases))
        self.assertEqual(selected, cases)
        self.assertFalse((ROOT / "docs/gui/gt710-step-22-8.json").exists())

    def test_step_seven_has_no_pending_body(self):
        source = (ROOT / "userspace/programs/src/suite_ui_testing.rs").read_text()
        category = None
        bodies = {}
        for line in source.splitlines():
            match = re.search(r'category\("([^"]+)"', line)
            if match:
                category = match.group(1)
            match = re.search(r'case\("([^"]+)".*', line)
            if match:
                bodies[f"{category}/{match.group(1)}"] = line
        selected = json.loads((ROOT / "docs/gui/gt710-step-22-7.json").read_text())["cases"]
        self.assertEqual(len(bodies), 108)
        self.assertEqual(list(bodies), selected)
        for case, body in bodies.items():
            with self.subTest(case=case):
                self.assertNotIn("pending(", body)
        self.assertNotRegex(source, r"fn\s+pending\s*\(")

    def test_step_six_bodies_and_pending_assignments(self):
        source = (ROOT / "userspace/programs/src/suite_ui_testing.rs").read_text()
        category = None
        bodies = {}
        for line in source.splitlines():
            match = re.search(r'category\("([^"]+)"', line)
            if match:
                category = match.group(1)
            match = re.search(r'case\("([^"]+)".*', line)
            if match:
                self.assertIsNotNone(category)
                bodies[f"{category}/{match.group(1)}"] = line
        manifest = json.loads((ROOT / "docs/suites/ui-testing.json").read_text())
        ids = {f"{cat['id']}/{case['id']}" for cat in manifest['categories'] for case in cat['cases']}
        self.assertEqual(set(bodies), ids)
        selected = json.loads((ROOT / "docs/gui/gt710-step-22-6.json").read_text())['cases']
        for case in selected:
            with self.subTest(case=case):
                self.assertNotIn('pending(', bodies[case])
        for case, body in bodies.items():
            if 'pending(' in body:
                with self.subTest(pending=case):
                    self.assertRegex(body, r'pending\("[^" ]+", 7\)')


if __name__ == '__main__':
    unittest.main()
