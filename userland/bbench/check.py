#!/usr/bin/env python3
"""Focused bbench check. Pass the native bbench executable."""
import json
import os
import pathlib
import subprocess
import sys
import tempfile

binary = pathlib.Path(sys.argv[1]).resolve()
with tempfile.TemporaryDirectory() as temp:
    root = pathlib.Path(temp)
    data, reports = root / "data", root / "reports"
    data.mkdir()
    reports.mkdir()

    def run(name, *args):
        process = subprocess.run(
            [binary, data, reports, name, *args], capture_output=True
        )
        report = (reports / f"{name}.json").read_bytes()
        assert report == process.stdout
        return process.returncode, json.loads(report)

    code, report = run("io", "sequential", "1048577")
    assert code == 0, report
    assert report["metrics"]["bytes_written"] == 1048577, report
    assert report["metrics"]["bytes_read"] == 1048577, report
    assert report["metrics"]["read_ns"] > 0, report
    assert not any(data.iterdir())

    # "Copy 1 GiB of bytes": two 128 MiB byte buffers copied eight times. The child's
    # peak resident size must fit those buffers (ru_maxrss is bytes on macOS).
    child = subprocess.Popen([binary, data, reports, "copy-bytes", "copy-bytes"], stdout=subprocess.PIPE)
    stdout = child.stdout.read()
    _, status, usage = os.wait4(child.pid, 0)
    report = json.loads(stdout)
    assert status == 0, report
    assert report["metrics"]["bytes_copied"] == 1 << 30, report
    peak = usage.ru_maxrss * (1 if sys.platform == "darwin" else 1024)
    assert peak < 384 << 20, f"copy peaked at {peak >> 20} MiB"

    code, report = run("failed-child", "command", str(root), "/usr/bin/false")
    assert code != 0
    assert "exit status: 1" in report["error"], report
    assert report["wall_ns"] > 0, report
