#!/usr/bin/env python3
"""Keep the composition display regression active on both architectures."""

import re
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SUITE = ROOT / "userspace/programs/src/suite_breengel"


class DisplayRegressionsTest(unittest.TestCase):
    def test_module_and_many_windows_are_unconditionally_enabled(self):
        main = (SUITE / "main.rs").read_text()
        declaration = re.search(r"(?m)^((?:\s*#\[[^\n]*\]\s*\n)*)\s*mod display_regressions\s*;", main)
        self.assertIsNotNone(declaration)
        self.assertNotRegex(declaration.group(1), r"#\s*\[\s*cfg(?:_attr)?\b")

        show = (SUITE / "show.rs").read_text()
        case = re.search(r"fn composition_fbinfo_geometry\(\) -> CaseResult\s*\{(.*?)\n\}", show, re.S)
        self.assertIsNotNone(case)
        self.assertNotRegex(case.group(1), r"#\s*\[\s*cfg(?:_attr)?\b")
        self.assertIn("crate::display_regressions::many_windows()?;", case.group(1))


if __name__ == "__main__":
    unittest.main()
