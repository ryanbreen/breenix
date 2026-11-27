#!/usr/bin/env python3
"""Focused bbench check. Pass the native bbench executable."""
import json
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

    code, report = run("failed-child", "command", str(root), "/usr/bin/false")
    assert code != 0
    assert "exit status: 1" in report["error"], report
    assert report["wall_ns"] > 0, report
