"""Pin the production window-manager client's completion-replay receipt contract."""
from pathlib import Path
import os
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class ReplayReceipt(unittest.TestCase):
    def test_reports_and_full_ring_rearm_are_required(self):
        source = (ROOT / "userspace/programs/src/wm_suite/input.rs").read_text()
        replay = source[source.index("pub fn replay("):]
        # Execute the validator verbatim, without changing the guest client or
        # substituting a second implementation of its acceptance predicate.
        validator = replay[replay.index("    let reports ="):replay.index("    pause();")]
        program = "fn validate(which: u32, request: [u8; 64]) -> Result<(), String> {\n"
        program += validator + "    Ok(())\n}\n"
        program += r'''
fn receipt(reports: u32, posted: u32, ring: u32) -> [u8; 64] {
    let mut bytes = [0; 64];
    for (at, value) in [(32, reports), (36, posted), (40, ring)] {
        bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }
    bytes
}

#[test]
fn normal_and_suppressed_keyboard_and_pointer_receipts() {
    for which in [1, 2, 0x101, 0x102] {
        let suppressed = which & 0x100 != 0;
        let reports = if suppressed { 0 } else { 1 };
        for ring in [16, 64] {
            assert!(validate(which, receipt(reports, ring, ring)).is_ok());
            if !suppressed {
                // The contract requires nonzero reports, not a fixed count.
                assert!(validate(which, receipt(2, ring, ring)).is_ok());
            }
            let wrong_reports = if suppressed { 1 } else { 0 };
            assert!(validate(which, receipt(wrong_reports, ring, ring)).is_err(),
                "selector {which:#x} accepted the wrong report count");
            for posted in [0, ring - 1, ring + 1] {
                assert!(validate(which, receipt(reports, posted, ring)).is_err(),
                    "selector {which:#x} accepted posted={posted}, ring={ring}");
            }
        }
        assert!(validate(which, receipt(reports, 0, 0)).is_err(),
            "selector {which:#x} accepted an empty ring");
    }
}
'''
        with tempfile.TemporaryDirectory(prefix="input-replay-receipt-") as temp:
            path, binary = Path(temp) / "check.rs", Path(temp) / "check"
            path.write_text(program)
            prefix = ["taskpolicy", "-b"] if sys.platform == "darwin" else []
            env = dict(os.environ, CARGO_BUILD_JOBS="4")
            result = subprocess.run(
                prefix + ["rustc", "--edition=2021", "--test", "-O", "-Dwarnings",
                          str(path), "-o", str(binary)],
                env=env, text=True, capture_output=True, timeout=60,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            result = subprocess.run(
                [str(binary)], text=True, capture_output=True, timeout=10,
            )
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)


if __name__ == "__main__":
    unittest.main()
