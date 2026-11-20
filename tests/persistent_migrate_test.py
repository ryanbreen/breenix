#!/usr/bin/env python3
"""Exercise the migrator's copy and read-back comparison on temporary trees."""
import hashlib
import os
from pathlib import Path
import stat
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
RECEIPT = b"[migrate] manifests equal\n"
MTIME = 1_234_567_890_123_456_789


class PersistentMigration(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        scratch = tempfile.TemporaryDirectory(prefix="persistent-migrate-test-")
        cls.addClassCleanup(scratch.cleanup)
        cls.driver = Path(scratch.name) / "copy"
        subprocess.run(
            [ROOT / "scripts/build-background.sh", "rustc", "--edition=2021",
             "-Dwarnings", ROOT / "tests/persistent_migrate_fixture.rs", "-o", cls.driver],
            check=True,
        )

    def fixture(self, root, users_layout=True):
        system = root / "ext2-system"
        generation = system / ".breenix-generations/fixture"
        shared = system / ".breenix-shared"
        for path in (root / "default", root / "home", root / "ext2-home", system / "root"):
            path.mkdir(parents=True)
        for directory in ("bin", "sbin", "usr/share/factory/etc", "root", "home", "dev", "proc",
                          "sys", "etc", "var", "opt", "srv", "tmp", "run"):
            (generation / directory).mkdir(parents=True)
        (generation / ".complete").write_text(
            "BREENIX-USERLAND 2\nfixture\nshared etc var opt srv tmp run\n"
        )
        (generation / "bin/tool").write_bytes(b"right")
        (generation / "bin/tool").chmod(0o4755)
        os.utime(generation / "bin/tool", ns=(MTIME, MTIME))
        (generation / "usr/share/factory/etc/policy").write_text("factory")
        os.symlink("../bin/tool", generation / "sbin/link")
        os.utime(generation / "sbin/link", ns=(MTIME, MTIME), follow_symlinks=False)
        (root / "ext2-home/profile").write_text("home")
        if users_layout:
            for directory in ("etc", "var", "opt", "srv", "tmp", "run"):
                (shared / directory).mkdir(parents=True)
            (shared / ".layout-v2").write_text("BREENIX-SHARED 2\n")
            policy = shared / "etc/policy\tactive"
            policy.write_text("machine")
            os.link(policy, system / "root/policy")
            os.mkfifo(shared / "run/pipe", 0o640)
            (shared / "tmp").chmod(0o1777)
            os.utime(shared / "tmp", ns=(MTIME, MTIME))
            (root / "ext2-home/sparse").write_bytes(b"a" + bytes(1024 * 1024) + b"b")
        return generation

    def test_copy_preserves_layout_metadata_hard_links_and_holes(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            self.fixture(root)
            result = subprocess.run([self.driver, root], capture_output=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(result.stdout, RECEIPT)
            source = (root / "evidence/source.tsv").read_bytes()
            self.assertEqual(source, (root / "evidence/destination.tsv").read_bytes())
            rows = [line.split(b"\t") for line in source.splitlines()]
            decoded = {bytes.fromhex(row[0].decode()): row[1:] for row in rows}
            self.assertEqual(list(decoded), sorted(decoded))
            tool = decoded[b"/gen-fixture/bin/tool"]
            self.assertEqual(tool[:2], [b"reg", b"4755"])
            self.assertEqual(tool[-3:], [
                b"5", b"1234567890.123456789", hashlib.sha256(b"right").hexdigest().encode()
            ])
            self.assertEqual(decoded[b"/gen-fixture/sbin/link"][-2], b"1234567890.123456789")
            self.assertEqual(decoded[b"/.breenix-shared/etc/policy\tactive"][4],
                             decoded[b"/root/policy"][4])
            self.assertNotEqual(decoded[b"/root/policy"][4], b"-")
            shared = root / "default/.breenix-shared"
            self.assertEqual((shared / "etc/policy\tactive").read_text(), "machine")
            self.assertEqual((root / "generation/usr/share/factory/etc/policy").read_text(), "factory")
            self.assertTrue(stat.S_ISFIFO((shared / "run/pipe").lstat().st_mode))
            self.assertEqual((shared / "tmp").stat().st_mtime_ns, MTIME)
            sparse = (root / "home/sparse").stat()
            self.assertEqual(sparse.st_size, 1024 * 1024 + 2)
            self.assertLess(sparse.st_blocks * 512, sparse.st_size)

    def test_planted_mismatch_exits_nonzero_without_receipt(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            self.fixture(root)
            result = subprocess.run([self.driver, root, "mismatch"], capture_output=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertNotIn(RECEIPT, result.stdout + result.stderr)
            self.assertNotEqual((root / "evidence/source.tsv").read_bytes(),
                                (root / "evidence/destination.tsv").read_bytes())

    def test_legacy_source_is_refused_before_snapshot_or_copy(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            self.fixture(root, users_layout=False)
            result = subprocess.run([self.driver, root], capture_output=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertNotIn(RECEIPT, result.stdout + result.stderr)
            self.assertIn(b"users layout", result.stderr)
            self.assertFalse((root / "generation").exists())
            self.assertEqual(list((root / "default").iterdir()), [])
            self.assertEqual(list((root / "home").iterdir()), [])


if __name__ == "__main__":
    unittest.main()
