#!/usr/bin/env python3
"""Keep ARM suite PID 1 routing aligned with the x86 launch contract."""

import pathlib
import re
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[1]


def routing(source, pattern, name_pattern):
    match = re.search(pattern, source, re.S)
    if not match:
        raise AssertionError("suite init routing predicate or argv not found")
    names = set(re.findall(name_pattern, match.group("predicate")))
    body = match.group("body")
    expected = [
        'alloc::vec![b"/sbin/init", b"suite", path.as_bytes()]',
        'alloc::vec![path.as_bytes()]',
        'b"--case", case.as_bytes()',
        'b"--hostkeys", value.as_bytes()',
    ]
    positions = [body.find(item) for item in expected]
    if any(position < 0 for position in positions) or positions != sorted(positions):
        raise AssertionError("suite argv must be init, suite, path, then case and hostkeys")
    return names


class SuiteInitRoutingTest(unittest.TestCase):
    def test_arm_matches_x86_suite_routing_and_arguments(self):
        x86 = (ROOT / "kernel/src/main.rs").read_text()
        arm = (ROOT / "kernel/src/main_aarch64.rs").read_text()
        x86_names = routing(
            x86,
            r'let mut argv: alloc::vec::Vec<&\[u8\]> = if (?P<predicate>.*?) \{(?P<body>.*?)let ticket = manager\.create_init_process_with_argv',
            r'name == "suite-([^"]+)"',
        )
        arm_names = routing(
            arm,
            r'let through_init = (?P<predicate>.*?);(?P<body>.*?)InitLaunch::new\(if through_init \{ "/sbin/init" \} else \{ &path \}, &argv\)',
            r'arg == Some\("([^"]+)"\)',
        )
        self.assertEqual(x86_names, {"users", "gui-apps"})
        self.assertEqual(arm_names, x86_names)


if __name__ == "__main__":
    unittest.main()
