import argparse
import contextlib
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
            record = {"sample": sample, "error": None}
            return subprocess.CompletedProcess(command, 0, stdout=json.dumps(record).encode())
        return subprocess.CompletedProcess(command, 0, stdout=b"")

    def test_remote_default_skips_sequential_and_walk(self):
        with patch.object(runner.subprocess, "run", side_effect=self.ssh), \
                patch.object(runner, "network_series") as network, \
                contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(runner.main(self.args), 0)
            self.assertIn("fork-exec", self.workloads)
            self.assertNotIn("sequential", self.workloads)
            self.assertNotIn("walk", self.workloads)
            network.assert_called_once()

            self.workloads.clear()
            self.args.cases = "sequential,walk"
            self.assertEqual(runner.main(self.args), 0)
            self.assertEqual(self.workloads, ["sequential", "walk"])


if __name__ == "__main__":
    unittest.main()
