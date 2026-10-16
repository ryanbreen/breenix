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

class ShowcasePaceBudgetTest(unittest.TestCase):
    def test_paced_start_keeps_strict_case_and_done_acceptance(self):
        import importlib.util
        import json
        spec = importlib.util.spec_from_file_location("verdict", ROOT / "scripts/suite-verdict.py")
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        manifest = json.loads((ROOT / "tests/fixtures/suite-verdict/manifest.json").read_text())
        manifest["id"] = "breengel"
        text = (ROOT / "tests/fixtures/suite-verdict/all-pass.txt").read_text().replace("SUITE fixture", "SUITE breengel")
        with tempfile.TemporaryDirectory() as directory:
            serial = Path(directory) / "serial.txt"
            for marker in ("400", "0", "4294967296", "-1", "400 bogus=1"):
                serial.write_text(text.replace("START cases=3", f"START cases=3 pace_ms={marker}"))
                self.assertEqual(module.verdict(manifest, serial, [serial])[0], marker == "400")
            serial.write_text(text.replace("START cases=3", "START cases=3 pace_ms=400").replace("PASS ms=1", "FAIL ms=1 msg=broken", 1))
            self.assertFalse(module.verdict(manifest, serial, [serial])[0])

    def test_completion_and_idle_budgets(self):
        import importlib.util
        spec = importlib.util.spec_from_file_location("pace", ROOT / "scripts/showcase-pace.py")
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        module.validate("breengel", 400, 393, 1800, 300)
        module.validate("breengel", 0, 393, 1800, 300)
        module.validate("breengel", 5000, 393, 2100, 300)
        for suite, pace, cases, deadline, idle in [
            ("breengel", 5000, 393, 1800, 300),
            ("breengel", 300000, 1, 1800, 300),
            ("breengel", 119000, 1, 1800, 300),
            ("files-io", 400, 393, 1800, 300),
        ]:
            with self.assertRaises(ValueError):
                module.validate(suite, pace, cases, deadline, idle)


if __name__ == "__main__":
    unittest.main()
