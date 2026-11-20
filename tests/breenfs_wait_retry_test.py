"""Run the kernel's BreenFS waits with forced refusal and PM owner migration."""
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class WaitRetry(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        fixture = (ROOT / "tests/fixtures/breenfs_wait_retry.rs").read_text()
        for marker, path in {
            "CACHE_PATH": "kernel/src/block/cache_kernel.rs",
            "SIGNAL_PATH": "kernel/src/fs/breenfs/commit_signal.rs",
            "MOUNT_PATH": "kernel/src/fs/breenfs/mount_lock.rs",
        }.items():
            fixture = fixture.replace(marker, str(ROOT / path))

        # Keep the decision and its load order from the kernel. The fixture
        # supplies only the CPU, IRQ mask and an owner register with a switch hook.
        process = (ROOT / "kernel/src/process/mod.rs").read_text()
        owner_queries = []
        for name in ("process_manager_owner_snapshot", "process_manager_held_on_current_cpu"):
            function = re.search(rf"(?ms)^pub fn {name}\(.*?^}}", process)
            assert function is not None, name
            owner_queries.append(function[0])
        fixture = fixture.replace("PM_OWNER_QUERIES", "\n".join(owner_queries))

        priority = ["taskpolicy", "-b"] if sys.platform == "darwin" else []
        tmp = tempfile.TemporaryDirectory()
        cls.addClassCleanup(tmp.cleanup)
        source = Path(tmp.name, "check.rs")
        cls.binary = Path(tmp.name, "check")
        source.write_text(fixture)
        subprocess.run(priority + ["rustc", "--edition=2021", "-Dwarnings",
                       str(source), "-o", str(cls.binary)], check=True, timeout=45)

    def run_scenario(self, scenario):
        return subprocess.run([self.binary, scenario], capture_output=True,
                              text=True, timeout=2)

    def test_waitqueue_refusal_leaves_the_cpu_available(self):
        for scenario in ("worker", "foreground", "mount", "cache",
                         "publish-refused", "epoch-moved", "killed-during-wake",
                         "killed-allocation-refused", "killed-mount-contender"):
            with self.subTest(scenario=scenario):
                result = self.run_scenario(scenario)
                self.assertEqual(result.returncode, 0, result.stderr)

    def test_process_manager_custody(self):
        with self.subTest(scenario="same-thread hold"):
            result = self.run_scenario("pm-held")
            self.assertEqual(result.returncode, 17, result.stderr)
            self.assertIn("block cache entered while holding the process manager", result.stderr)
        with self.subTest(scenario="migration after owner load"):
            result = self.run_scenario("pm-migrate")
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(result.stderr, "")


if __name__ == "__main__":
    unittest.main()
