"""The CASE hash field is a strict extension, not an unparsed serial suffix."""
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("verdict", ROOT / "scripts/suite-verdict.py")
scorer = importlib.util.module_from_spec(spec)
spec.loader.exec_module(scorer)
fixture = ROOT / "tests/fixtures/suite-verdict"
manifest = json.loads((fixture / "manifest.json").read_text())


class JudgedHashTests(unittest.TestCase):
    def score(self, suffix):
        text = (fixture / "all-pass.txt").read_text().replace("PASS ms=1", "PASS ms=1" + suffix)
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "serial.txt"
            path.write_text(text)
            return scorer.verdict(manifest, path, [path])

    def test_complete_hash_is_accepted(self):
        self.assertTrue(self.score(" sha256=" + "a1" * 32)[0])

    def test_malformed_hash_is_rejected(self):
        for suffix in [" sha256=", " sha256=" + "a" * 63, " sha256=" + "a" * 65,
                       " sha256=" + "gg" * 32, " sha256=" + "AA" * 32,
                       " arbitrary=1", " sha256=" + "01" * 32 + " extra"]:
            with self.subTest(suffix=suffix):
                self.assertFalse(self.score(suffix)[0])


if __name__ == "__main__":
    unittest.main()
