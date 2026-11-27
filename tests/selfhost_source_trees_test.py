"""Mac self-host receipts must describe the current tool source trees."""
import json
from pathlib import Path
import subprocess
import unittest

ROOT = Path(__file__).resolve().parent.parent
TOOLS = ("bobj", "bas", "bld", "bar", "bnm", "bmake", "bcc")


class SelfhostSourceTrees(unittest.TestCase):
    def test_receipts_match_head(self):
        for receipt_path in sorted((ROOT / "toolchain/rust-host/selfhost").glob("*-mac.json")):
            receipt = json.loads(receipt_path.read_text())
            for tool in TOOLS:
                with self.subTest(receipt=receipt_path.name, tool=tool):
                    actual = subprocess.check_output(
                        ["git", "rev-parse", f"HEAD:toolchain/{tool}"], cwd=ROOT, text=True
                    ).strip()
                    self.assertEqual(receipt["workspace"][tool], actual)


if __name__ == "__main__":
    unittest.main()
