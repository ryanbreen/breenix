#!/usr/bin/env python3
"""Exercise boot commit identity in Git checkouts and deployed archives."""

import io
from pathlib import Path
import subprocess
import tarfile
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
HELPER = ROOT / "scripts/boot-commit.sh"


def identity(tree):
    return subprocess.run(
        ["bash", "-c", 'source "$1"; boot_commit', "bash", str(HELPER)],
        cwd=tree, text=True, capture_output=True, check=False,
    )


class BootCommitTest(unittest.TestCase):
    def test_git_tree_clean_and_dirty(self):
        with tempfile.TemporaryDirectory() as tmp:
            tree = Path(tmp)
            subprocess.run(["git", "init", "-q", str(tree)], check=True)
            subprocess.run(["git", "-C", str(tree), "-c", "user.name=Test",
                            "-c", "user.email=test@example.invalid", "commit",
                            "-q", "--allow-empty", "-m", "fixture"], check=True)
            sha = subprocess.check_output(["git", "-C", str(tree), "rev-parse", "HEAD"], text=True).strip()
            result = identity(tree)
            self.assertEqual((result.returncode, result.stdout), (0, f"{sha} dirty=0\n"))
            (tree / "change").write_text("dirty\n")
            result = identity(tree)
            self.assertEqual((result.returncode, result.stdout), (0, f"{sha} dirty=1\n"))

    def test_archived_tree_identity(self):
        sha = subprocess.check_output(["git", "-C", str(ROOT), "rev-parse", "HEAD"], text=True).strip()
        archive = subprocess.check_output(["git", "-C", str(ROOT), "archive", "HEAD"])
        with tempfile.TemporaryDirectory() as tmp:
            tree = Path(tmp)
            with tarfile.open(fileobj=io.BytesIO(archive)) as tar:
                tar.extractall(tree, filter="data")
            marker = tree / ".deployed-commit"
            marker.write_text(sha + "\n")
            result = identity(tree)
            self.assertEqual((result.returncode, result.stdout), (0, f"{sha} deployed=1\n"))
            for contents in (None, "not-a-commit\n"):
                if contents is None:
                    marker.unlink()
                else:
                    marker.write_text(contents)
                result = identity(tree)
                self.assertEqual(result.returncode, 2)
                self.assertEqual(result.stdout, "")
                self.assertIn("no commit identity", result.stderr)


if __name__ == "__main__":
    unittest.main()
