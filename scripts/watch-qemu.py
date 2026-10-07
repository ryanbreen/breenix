#!/usr/bin/env python3
"""Run QEMU until it exits, serial goes idle, a fatal occurs, or a suite finishes."""

import argparse
import json
from pathlib import Path
import re
import runpy
import signal
import subprocess
import time

ROOT = Path(__file__).resolve().parents[1]
DONE = runpy.run_path(str(ROOT / "scripts/wait-boot-done.py"))
VERDICT = runpy.run_path(str(ROOT / "scripts/suite-verdict.py"))


def stop(process):
    if process.poll() is None:
        process.terminate()
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            process.kill()
    process.wait()


def watch(process, serial, mode, suite, idle_exit):
    last_size = 0
    last_output = time.monotonic()
    while process.poll() is None:
        try:
            data = serial.read_bytes()
        except FileNotFoundError:
            data = b""
        if len(data) != last_size:
            last_size = len(data)
            last_output = time.monotonic()
        text = data.decode("utf-8", errors="replace")
        for line in text.splitlines():
            if any(re.search(pattern, line) for pattern in DONE["FATAL"]):
                print(f"FAIL: fatal kernel output: {line}", flush=True)
                return "fatal"
        if mode == "suite" and DONE["completion"](text, mode, suite) is not None:
            print(f"==> Suite {suite} completed; stopping the VM", flush=True)
            return "done"
        if idle_exit > 0 and time.monotonic() - last_output >= idle_exit:
            print(f"==> No serial output for {idle_exit:g}s; stopping the VM", flush=True)
            return "idle"
        time.sleep(0.2)
    return "exit"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--serial", type=Path, required=True)
    parser.add_argument("--mode", required=True)
    parser.add_argument("--suite", default="")
    parser.add_argument("--idle-exit", type=float, default=300)
    parser.add_argument("--disk")
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    command = args.command[1:] if args.command[:1] == ["--"] else args.command
    if not command:
        parser.error("a QEMU command is required after --")

    # Cleanup is synchronous, including signals sent only to this watcher.
    def interrupted(signum, frame):
        raise SystemExit(128 + signum)

    for signum in (signal.SIGTERM, signal.SIGHUP, signal.SIGINT):
        signal.signal(signum, interrupted)
    process = subprocess.Popen(command)
    try:
        reason = watch(process, args.serial, args.mode, args.suite, args.idle_exit)
    finally:
        stop(process)
    if reason == "fatal":
        return 1
    if args.mode == "suite":
        serial = args.serial.read_text(errors="replace") if args.serial.exists() else ""
        result = DONE["completion"](serial, args.mode, args.suite)
        if result is None:
            print(f"FAIL: no complete SUITE {args.suite} DONE before {reason}", flush=True)
            return 1
        if not result[0]:
            print(f"FAIL: {result[1]}", flush=True)
            return 1
        manifest = json.loads((ROOT / f"docs/suites/{args.suite}.json").read_text())
        ok, detail = VERDICT["verdict"](manifest, args.serial, [args.serial])
        if ok:
            ok, disk_detail = VERDICT["disk_verdict"](manifest.get("diskChecks", []), args.disk)
            if disk_detail:
                detail += f"; {disk_detail}"
        print(f"{'PASS' if ok else 'FAIL'}: {detail}", flush=True)
        return 0 if ok else 1
    return process.returncode if process.returncode >= 0 else 128 - process.returncode


if __name__ == "__main__":
    raise SystemExit(main())
