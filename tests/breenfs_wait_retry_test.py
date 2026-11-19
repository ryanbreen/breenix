"""Run the kernel's BreenFS wait callers with forced waitqueue refusal."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class WaitRetry(unittest.TestCase):
    def test_waitqueue_refusal_leaves_the_cpu_available(self):
        fixture = (ROOT / "tests/fixtures/breenfs_wait_retry.rs").read_text()
        for marker, path in {
            "CACHE_PATH": "kernel/src/block/cache_kernel.rs",
            "SIGNAL_PATH": "kernel/src/fs/breenfs/commit_signal.rs",
            "MOUNT_PATH": "kernel/src/fs/breenfs/mount_lock.rs",
        }.items():
            fixture = fixture.replace(marker, str(ROOT / path))

        priority = ["taskpolicy", "-b"] if sys.platform == "darwin" else []
        with tempfile.TemporaryDirectory() as tmp:
            source = Path(tmp, "check.rs")
            binary = Path(tmp, "check")
            source.write_text(fixture)
            subprocess.run(priority + ["rustc", "--edition=2021", "-Dwarnings",
                           str(source), "-o", str(binary)], check=True, timeout=45)
            for scenario in ("worker", "foreground", "mount", "cache",
                             "publish-refused", "epoch-moved"):
                with self.subTest(scenario=scenario):
                    result = subprocess.run([binary, scenario], capture_output=True,
                                            text=True, timeout=2)
                    self.assertEqual(result.returncode, 0, result.stderr)


if __name__ == "__main__":
    unittest.main()
