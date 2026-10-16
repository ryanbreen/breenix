"""Exercise watch pacing on real ext2 boot disks without starting a VM."""
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class ShowcasePaceTest(unittest.TestCase):
    def test_pace_is_readable_and_removed_by_an_unpaced_target(self):
        debugfs = shutil.which("debugfs") or "/opt/homebrew/opt/e2fsprogs/sbin/debugfs"
        mke2fs = shutil.which("mke2fs") or "/opt/homebrew/opt/e2fsprogs/sbin/mke2fs"
        with tempfile.TemporaryDirectory() as directory:
            work = Path(directory)
            files = work / "files"
            (files / "sbin").mkdir(parents=True)
            (files / "etc").mkdir()
            binary = files / "sbin/suite-breengel"
            binary.write_bytes(b"PID 1 fixture for boot-disk byte verification")
            disk = work / "boot.img"
            with disk.open("wb") as output:
                output.truncate(8 * 1024 * 1024)
            subprocess.run([mke2fs, "-q", "-F", "-t", "ext2", "-d", str(files), str(disk)],
                           check=True, capture_output=True)

            def read(path):
                return subprocess.run([debugfs, "-R", f"cat {path}", str(disk)],
                                      check=True, capture_output=True, text=True).stdout

            for pace in ("400", "0", "4294967295", ""):
                subprocess.run([str(ROOT / "scripts/write-boot-target.sh"), str(disk),
                                "breengel", str(binary), "", "", pace],
                               check=True, capture_output=True, text=True)
                self.assertEqual(read("/etc/breenix/boot-target"), "suite breengel\n")
                self.assertEqual(read("/etc/breenix/pace"), pace + "\n" if pace else "")
                self.assertEqual(read("/sbin/suite-breengel").encode(), binary.read_bytes())
            for invalid in ("-1", "1.5", "4294967296", "abc"):
                result = subprocess.run([str(ROOT / "scripts/write-boot-target.sh"), str(disk),
                                         "breengel", str(binary), "", "", invalid],
                                        capture_output=True, text=True)
                self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
                self.assertEqual(read("/etc/breenix/pace"), "")


if __name__ == "__main__":
    unittest.main()
