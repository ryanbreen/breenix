import argparse
import contextlib
from datetime import datetime, timezone
import hashlib
import io
import json
from pathlib import Path
import shlex
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

import run as runner


class RemoteSeries(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.args = argparse.Namespace(
            host="developer@bench-box", mac_ssh=None, source="/home/developer/src",
            data="/home/developer/cache", driver="/opt/driver.so", run="review",
            out=Path(temp.name), probe="/home/developer/bin/bbench",
            bcargo="/home/developer/bin/bcargo", bsync="bsync", noop_target=None,
            cases=None, count=1,
        )
        self.workloads = []

    def ssh(self, command, **kwargs):
        self.assertEqual(command[0], "/usr/bin/ssh")
        words = shlex.split(command[-1])
        if words == ["/bin/cat", "/opt/breenix/rust/CURRENT"]:
            return subprocess.CompletedProcess(command, 0, stdout=b"toolchain-id\n")
        if words[0] == "/bin/test":
            return subprocess.CompletedProcess(command, 1)
        if words[0] == "/bin/env":
            sample, workload = words[5:7]
            self.workloads.append(workload)
            record = {"sample": sample, "error": None, "wall_ns": 1_000_000_000,
                      "metrics": {field: 1_000_000 for field in (
                          "first_output_ns", "map_ns", "touch_ns", "copy_ns",
                          "write_ns", "read_ns", "create_ns", "stat_ns", "unlink_ns",
                          "walk_ns", "fsync_ns",
                      )}}
            return subprocess.CompletedProcess(command, 0, stdout=json.dumps(record).encode())
        return subprocess.CompletedProcess(command, 0, stdout=b"")

    def test_remote_default_skips_deferred_disk_workloads(self):
        with patch.object(runner.subprocess, "run", side_effect=self.ssh), \
                patch.object(runner, "network_series") as network, \
                contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(runner.main(self.args), 0)
            self.assertIn("fork-exec", self.workloads)
            self.assertNotIn("sequential", self.workloads)
            self.assertNotIn("small-files", self.workloads)
            self.assertNotIn("walk", self.workloads)
            self.assertNotIn("fsync", self.workloads)
            network.assert_called_once()

            self.workloads.clear()
            self.args.cases = "sequential,small-files,walk,fsync"
            self.assertEqual(runner.main(self.args), 0)
            self.assertEqual(self.workloads, ["sequential", "small-files", "walk", "fsync"])

    def test_network_resolves_alias_before_running_workloads(self):
        self.args.host = "bench-box"
        self.args.cases = "fork-exec,network"
        payload = b"x" * 8_000_000
        digest = hashlib.sha256(payload).hexdigest()
        commands = []

        def execute(command, **kwargs):
            commands.append(command)
            if "-G" in command:
                self.assertEqual(command[-1], "bench-box")
                return subprocess.CompletedProcess(
                    command, 0, stdout="user developer\nhostname configured.example\nport 2222\n"
                )
            if command[0] == "bsync":
                self.assertEqual((self.args.out / "network/payload").stat().st_size, len(payload))
                self.assertEqual(command[-1], "developer@bench-box:/home/developer/cache/review/network/1")
                return subprocess.CompletedProcess(command, 0)
            if command[0] == "/usr/bin/scp":
                Path(command[-1]).write_bytes(payload)
                return subprocess.CompletedProcess(command, 0)
            if command[-1].startswith("/bin/sha256sum "):
                return subprocess.CompletedProcess(command, 0, stdout=f"{digest}  payload\n".encode())
            return self.ssh(command, **kwargs)

        with patch.object(runner.subprocess, "run", side_effect=execute), \
                patch.object(runner.os, "urandom", side_effect=lambda count: b"x" * count), \
                contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(runner.main(self.args), 0)

        self.assertIn("-G", commands[0])
        receipt = json.loads((self.args.out / "review-bsync-upload-1.json").read_bytes())
        self.assertEqual(receipt["metrics"], {"bytes": len(payload), "sha256": digest})

    def test_driver_emits_series_and_converter_reproduces_saved_baseline(self):
        bench = Path(__file__).resolve().parent
        baseline = {}
        for line in (bench / "BASELINE.md").read_text().splitlines():
            if line.startswith("| ") and not line.startswith(("| ---", "| Workload")):
                title, box, *_ = (column.strip() for column in line.strip("|").split("|"))
                baseline[title] = box.split()[0]

        def read_series(path):
            series = json.loads(path.read_bytes())
            self.assertEqual(set(series), {
                "version", "id", "at", "system", "target", "hardware", "kernelCommit",
                "userlandCommit", "toolchain", "session", "pr", "note", "rows",
            })
            self.assertEqual(series["version"], 1)
            self.assertEqual(datetime.fromisoformat(series["at"]).utcoffset(),
                             timezone.utc.utcoffset(None))
            rows = {row["id"]: row for row in series["rows"]}
            self.assertEqual(len(rows), len(series["rows"]))
            self.assertEqual({row["title"] for row in rows.values()}, set(baseline))
            for row in rows.values():
                self.assertEqual(set(row), {
                    "id", "title", "area", "unit", "better", "state", "median",
                    "min", "max", "completed", "attempts", "cache", "reason",
                })
                if row["state"] == "measured":
                    self.assertLessEqual(row["min"], row["median"])
                    self.assertLessEqual(row["median"], row["max"])
                    self.assertGreater(row["completed"], 0)
                else:
                    self.assertEqual((row["median"], row["min"], row["max"]), (None,) * 3)
                    self.assertEqual(row["completed"], 0)
                self.assertLessEqual(row["completed"], row["attempts"])
            return series, rows

        with self.subTest("driver"):
            self.args.cases = "rustc-version"
            with patch.object(runner.subprocess, "run", side_effect=self.ssh), \
                    contextlib.redirect_stdout(io.StringIO()):
                self.assertEqual(runner.main(self.args), 0)
            series, rows = read_series(self.args.out / "review.series.json")
            self.assertEqual((series["id"], series["system"], series["target"], series["toolchain"]),
                             ("review", "breenix", "x86-metal", "toolchain-id"))
            self.assertEqual(rows["compile.rustc-vv"]["median"], 1)
            self.assertEqual(rows["process.rustc-first-output"]["median"], .001)
            for row in rows.values():
                if row["area"] == "disk":
                    self.assertEqual((row["state"], row["reason"], row["attempts"]),
                                     ("deferred", "#3226", 0))

        with self.subTest("converter"):
            receipts = self.args.out / "saved"
            receipts.mkdir()
            for sample in json.loads((bench / "fixtures/r240-dev25.json").read_bytes()):
                (receipts / f"{sample['sample']}.json").write_text(json.dumps(sample))
            subprocess.run([
                sys.executable, bench / "run.py", "series-from-receipts", receipts,
                "--run", "r240-20261127-dev25", "--out", self.args.out,
                "--at", "2026-11-27T19:10:19Z", "--system", "breenix",
                "--target", "x86-metal", "--hardware", "Dell PowerEdge R240",
                "--kernel-commit", "bbb69a99", "--userland-commit", "e6bbc8d4",
            ], check=True, capture_output=True)
            series, rows = read_series(self.args.out / "r240-20261127-dev25.series.json")
            self.assertEqual((series["kernelCommit"], series["userlandCommit"]),
                             ("bbb69a99", "e6bbc8d4"))
            self.assertEqual((rows["compile.bcc"]["state"], rows["compile.bcc"]["attempts"]),
                             ("failed", 1))
            for row in rows.values():
                expected = baseline[row["title"]]
                if expected[0].isdigit():
                    precision = len(expected.split(".")[1])
                    self.assertEqual(f"{row['median']:.{precision}f}", expected, row["id"])
                else:
                    self.assertIsNone(row["median"], row["id"])


if __name__ == "__main__":
    unittest.main()
