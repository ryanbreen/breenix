import argparse
import contextlib
import hashlib
import io
import json
from pathlib import Path
import shlex
import subprocess
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
            out=Path(temp.name), probe="/home/developer/bin/bbench", wrap_rustc=False,
            bcargo="/home/developer/bin/bcargo", bsync="bsync", noop_target=None,
            cases=None, count=1,
        )
        self.workloads = []
        self.workload_args = []

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
            self.workload_args.append(words[6:])
            record = {"sample": sample, "error": None}
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

    def test_malloc_series_dispatches_one_and_four_workers(self):
        self.args.cases = "malloc-1t,malloc-4t"
        with patch.object(runner.subprocess, "run", side_effect=self.ssh), \
                contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(runner.main(self.args), 0)
        self.assertEqual(self.workload_args, [["malloc-free", "1"], ["malloc-free", "4"]])

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


if __name__ == "__main__":
    unittest.main()
