"""The series writer's attribution rows: units, direction and per-row reasons."""
import json
from pathlib import Path
import tempfile
import unittest

import rig
import series
import tables


def receipt(sample, error=None, **metrics):
    return {"sample": sample, "error": error, "wall_ns": 5 * 10**9, "metrics": metrics}


class AttributionRows(unittest.TestCase):
    def write(self, groups, **kwargs):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        metadata = {"system": "linux", "target": "arm64-qemu", "hardware": "virt",
                    "kernelCommit": None, "userlandCommit": "abc", "toolchain": None,
                    "session": None, "pr": None, "note": None}
        path = series.write_series(Path(temp.name), "rig", "2026-11-27T22:00:00Z",
                                   metadata, groups, **kwargs)
        return {row["id"]: row for row in json.loads(path.read_bytes())["rows"]}

    def test_throughput_rows_are_higher_is_better_megabytes_per_second(self):
        rows = self.write({"tcp-send": [
            receipt("rig-tcp-send-1", tcp_ns=10**9, bytes=32_000_000),
            receipt("rig-tcp-send-2", tcp_ns=2 * 10**9, bytes=32_000_000),
            receipt("rig-tcp-send-3", error="tcp send moved 0 of 32000000 bytes"),
        ]})
        row = rows["network.tcp-guest-to-host"]
        self.assertEqual((row["unit"], row["better"], row["area"]), ("MB/s", "higher", "network"))
        self.assertEqual((row["median"], row["min"], row["max"]), (24.0, 16.0, 32.0))
        self.assertEqual((row["completed"], row["attempts"]), (2, 3))

    def test_crypto_rows_keep_each_opt_level_apart(self):
        rows = self.write({
            "aes-gcm-oz": [receipt("rig-aes-gcm-oz-1", crypto_ns=4 * 10**9, bytes=8_000_000)],
            "aes-gcm-o3": [receipt("rig-aes-gcm-o3-1", crypto_ns=10**9, bytes=8_000_000)],
        })
        self.assertEqual(rows["ssh.aes256-gcm-oz"]["median"], 2.0)
        self.assertEqual(rows["ssh.aes256-gcm-o3"]["median"], 8.0)
        self.assertEqual(rows["ssh.aes256-gcm-o3"]["better"], "higher")
        self.assertEqual(rows["ssh.hmac-sha256-o3"]["state"], "deferred")

    def test_call_rows_are_lower_is_better_milliseconds(self):
        rows = self.write({
            "getpid": [receipt("rig-getpid-1", getpid_ns=250 * 10**6)],
            "malloc-4t": [receipt("rig-malloc-4t-1", malloc_ns=40 * 10**6)],
        })
        self.assertEqual((rows["process.getpid"]["median"], rows["process.getpid"]["unit"],
                          rows["process.getpid"]["better"]), (250.0, "ms", "lower"))
        self.assertEqual(rows["memory.malloc-4t"]["median"], 40.0)

    def test_rows_a_system_cannot_run_carry_the_given_reason(self):
        reason = "No aarch64 Breenix-hosted rustc in the standard image"
        rows = self.write({}, deferred={"rustc-version": reason})
        self.assertEqual((rows["compile.rustc-vv"]["state"], rows["compile.rustc-vv"]["reason"]),
                         ("deferred", reason))
        self.assertEqual(rows["process.rustc-first-output"]["reason"], reason)
        self.assertEqual(rows["memory.anonymous-touch"]["reason"], "No timed receipts")


class CopyRows(unittest.TestCase):
    """The probe copied 4 GiB (i32 buffers) until the rig's branch, then 1 GiB of bytes.
    The two measure different work, so they are different rows."""

    def receipts(self, *names):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        for name in names:
            Path(temp.name, f"{name}.json").write_text(json.dumps(
                receipt(name, copy_ns=10**9, bytes_copied=1 << 30)))
        return tables.collect_receipts(temp.name, ["rig"], "scp-upload")

    def write(self, groups):
        return AttributionRows.write(self, groups)

    def test_the_byte_copy_has_its_own_row(self):
        rows = self.write(self.receipts("rig-copy-bytes-1", "rig-copy-bytes-2"))
        self.assertEqual((rows["memory.copy-bytes"]["completed"],
                          rows["memory.copy-bytes"]["median"]), (2, 1.0))
        self.assertIn("1 GiB", rows["memory.copy-bytes"]["title"])
        self.assertEqual(rows["memory.copy"]["completed"], 0)

    def test_historical_copy_receipts_stay_on_the_4_gib_row(self):
        rows = self.write(self.receipts("rig-copy-1"))
        self.assertEqual(rows["memory.copy"]["completed"], 1)
        self.assertIn("4 GiB", rows["memory.copy"]["title"])
        self.assertEqual(rows["memory.copy-bytes"]["completed"], 0)

    def test_a_new_series_says_why_the_4_gib_row_is_empty(self):
        rows = self.write({})
        self.assertEqual(rows["memory.copy"]["state"], "deferred")
        self.assertIn("memory.copy-bytes", rows["memory.copy"]["reason"])

    def test_the_rig_runs_the_byte_copy(self):
        self.assertIn("copy-bytes", rig.CASES)
        self.assertNotIn("copy", rig.CASES)


if __name__ == "__main__":
    unittest.main()
