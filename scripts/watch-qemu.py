#!/usr/bin/env python3
"""Run QEMU until it exits, serial goes idle, a fatal occurs, or a suite finishes."""

import argparse
import codecs
import json
from pathlib import Path
import os
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


# Modes that print a completion record. Their kernels keep logging after it (the
# periodic census), so the idle timer alone would leave a finished VM running.
COMPLETING_MODES = ("suite", "probe", "tests")


def label(mode, suite):
    return f"Suite {suite}" if mode == "suite" else "Boot probe" if mode == "probe" else "Testing boot"


def watch(process, serial, mode, suite, idle_exit, gate_timeout, suite_hold):
    last_size = 0
    started = last_output = time.monotonic()
    done_at = None
    text = ""
    decoder = codecs.getincrementaldecoder("utf-8")(errors="replace")
    while process.poll() is None:
        now = time.monotonic()
        try:
            size = serial.stat().st_size
        except FileNotFoundError:
            size = 0
        if size != last_size:
            if size < last_size:
                last_size = 0
                text = ""
                decoder.reset()
            with serial.open("rb") as source:
                source.seek(last_size)
                data = source.read()
            last_size += len(data)
            last_output = now
            if mode in COMPLETING_MODES:
                text += decoder.decode(data)
                result = DONE["completion"](text, mode, suite)
                if result is not None:
                    if result[1].startswith("fatal kernel output:"):
                        print(f"FAIL: {result[1]}", flush=True)
                        return "fatal"
                    if done_at is None:
                        done_at = now
                        print(f"==> {label(mode, suite)} DONE observed; holding the final screen", flush=True)
        # Match the x86 gate's render delay and configurable panel hold, while
        # continuing to observe fatal output and additional completion records.
        if done_at is not None:
            if now - done_at >= 2 + suite_hold:
                print(f"==> {label(mode, suite)} completed; stopping the VM", flush=True)
                return "done"
        elif mode == "suite" and gate_timeout > 0 and now - started >= gate_timeout:
            print(f"==> No suite DONE within {gate_timeout:g}s; stopping the VM", flush=True)
            return "deadline"
        elif idle_exit > 0 and now - last_output >= idle_exit:
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
    parser.add_argument("--gate-timeout", type=float, default=1800)
    parser.add_argument("--suite-hold", type=float, default=float(os.environ.get("BREENIX_SUITE_HOLD", "5")))
    parser.add_argument("--disk")
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    if args.idle_exit < 0 or args.gate_timeout <= 0 or args.suite_hold < 0:
        parser.error("idle exit and suite hold must be nonnegative; gate timeout must be positive")
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
        reason = watch(process, args.serial, args.mode, args.suite, args.idle_exit,
                       args.gate_timeout, args.suite_hold)
    finally:
        stop(process)
    if reason == "fatal":
        return 1
    if args.mode in ("probe", "tests") and reason == "done":
        serial = args.serial.read_bytes().decode("utf-8", errors="replace")
        result = DONE["completion"](serial, args.mode, args.suite)
        print(f"{'PASS' if result and result[0] else 'FAIL'}: {result[1] if result else 'no completion record'}", flush=True)
        return 0 if result and result[0] else 1
    if args.mode == "suite":
        serial = args.serial.read_bytes().decode("utf-8", errors="replace") if args.serial.exists() else ""
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
