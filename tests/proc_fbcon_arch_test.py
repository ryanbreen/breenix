#!/usr/bin/env python3
"""The framebuffer console proc interface must be available on ARM and x86."""

import re
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
CONSOLE = ROOT / "kernel/src/console/mod.rs"
PROCFS = ROOT / "kernel/src/fs/procfs/mod.rs"


class FbconArchTest(unittest.TestCase):
    def test_fbcon_items_are_not_x86_only(self):
        for path, pattern in (
            (CONSOLE, r"pub fn proc_fbcon_check\b"),
            (PROCFS, r"\bFbcon(?:Dir|Check)\b"),
        ):
            source = path.read_text()
            matches = list(re.finditer(pattern, source))
            self.assertGreater(len(matches), 1 if path == PROCFS else 0)
            for match in matches:
                line = source.count("\n", 0, match.start()) + 1
                preceding = source[:match.start()].splitlines()
                # An attribute can govern an item directly or an enclosing block.
                for index in range(len(preceding) - 1, -1, -1):
                    text = preceding[index].strip()
                    if text.startswith("#[cfg") and 'target_arch' in text and '"x86_64"' in text:
                        between = "\n".join(preceding[index + 1:])
                        if not between.strip() or (between.strip().startswith("{") and between.count("{") > between.count("}")):
                            self.fail(f"{path.relative_to(ROOT)}:{line} is gated by {text}")
                    if text and not text.startswith(("#", "//")) and text not in ("{",):
                        # A containing cfg block may still surround this item.
                        if text.count("}") > text.count("{"):
                            break

    def test_fbcon_inodes_are_unique(self):
        source = PROCFS.read_text()
        assignments = re.findall(r"ProcEntryType::(\w+)\s*=>\s*(\d+)\s*,", source)
        for name, inode in (("FbconDir", "500"), ("FbconCheck", "501")):
            self.assertEqual([entry for entry, number in assignments if number == inode], [name])


if __name__ == "__main__":
    unittest.main()
