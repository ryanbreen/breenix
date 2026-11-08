#!/usr/bin/env python3
"""Exercise the reference installer used by both image creation paths."""
import hashlib
import json
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]

class ReferenceInstall(unittest.TestCase):
    def test_guest_files_and_pin(self):
        with tempfile.TemporaryDirectory() as directory:
            subprocess.run(["bash", str(ROOT / "scripts/install-ui-references.sh"),
                            str(ROOT / "docs/suites/frames"), directory], check=True)
            installed = Path(directory) / "usr/share/breenix/uitest/frames"
            png = installed / "ui-testing/coordinate-pattern.png"
            provenance = json.loads((installed / "PROVENANCE.json").read_text())
            row, = [r for r in provenance["references"] if r["path"] == "ui-testing/coordinate-pattern.png"]
            self.assertEqual(hashlib.sha256(png.read_bytes()).hexdigest(), row["sha256"])
            self.assertEqual(png.read_bytes(), (ROOT / "docs/suites/frames/ui-testing/coordinate-pattern.png").read_bytes())
            script = (ROOT / "scripts/create_ext2_disk.sh").read_text()
            self.assertIn('/install-ui-references.sh /suite-frames /mnt/ext2', script)
            self.assertIn('"$PROJECT_ROOT/scripts/install-ui-references.sh" "$PROJECT_ROOT/docs/suites/frames" "$MOUNT_DIR"', script)

if __name__ == "__main__":
    unittest.main()
