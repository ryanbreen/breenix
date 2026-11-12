#!/usr/bin/env python3
"""Run QEMU until it exits, serial goes idle, a fatal occurs, or a suite finishes."""

import argparse
import codecs
import json
from pathlib import Path
import os
import re
import runpy
import signal
import subprocess
import time

WallClockCheck = runpy.run_path(str(Path(__file__).with_name("wall_clock.py")))["WallClockCheck"]
BootProgress = runpy.run_path(str(Path(__file__).with_name("boot_progress.py")))["BootProgress"]

SerialClock = runpy.run_path(str(Path(__file__).with_name("serial-clock.py")))["SerialClock"]

CaseSamples = runpy.run_path(str(Path(__file__).with_name("qemu-case-sample.py")))["CaseSamples"]

ROOT = Path(__file__).resolve().parents[1]
DONE = runpy.run_path(str(ROOT / "scripts/wait-boot-done.py"))
VERDICT = runpy.run_path(str(ROOT / "scripts/suite-verdict.py"))
SEQUENCE = runpy.run_path(str(ROOT / "scripts/suite-sequence.py"))


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
COMPLETING_MODES = ("suite", "probe", "tests", "program")


def label(mode, suite):
    if mode == "suite" and "," in suite:
        return f"Suites {suite}"
    return f"Suite {suite}" if mode == "suite" else "Boot probe" if mode == "probe" else "Testing boot"


def watch(process, serial, mode, suite, idle_exit, gate_timeout, suite_hold, clock, only=None,
          until_desktop_ready=False, serial_clock=None, case_samples=None):
    progress = BootProgress(serial)
    serial_clock = serial_clock or SerialClock(serial)
    desktop_stages = []
    if until_desktop_ready:
        catalog = json.loads((ROOT / "docs/boot-path.json").read_text())
        desktop_stages = [set(stage["marker"].split("|"))
                          for milestone in catalog["milestones"]
                          if milestone["id"] in ("first-userspace", "runtimes")
                          for stage in milestone["stages"]["desktop"]]
    last_size = 0
    started = last_output = time.monotonic()
    boundaries = 0
    done_at = None
    text = ""
    decoder = codecs.getincrementaldecoder("utf-8")(errors="replace")
    while process.poll() is None:
        serial_clock.observe()
        if case_samples:
            case_samples.tick()
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
            chunk = decoder.decode(data)
            if case_samples:
                case_samples.observe(chunk)
            text += chunk
            progress.observe(text)
            if until_desktop_ready:
                fatal = DONE["completion"](text, "desktop", "")
                failed = next((line for line in text.splitlines()
                               if re.search(r"\[bwm\].*\bFAILED\b", line)), None)
                if fatal or failed:
                    print(f"FAIL: {fatal[1] if fatal else failed}", flush=True)
                    return "fatal"
                if all(stage & progress.stages for stage in desktop_stages):
                    print("PASS: desktop ready; stopping the VM", flush=True)
                    return "done"
            if mode in COMPLETING_MODES:
                clock.observe(text)
                if mode == "suite" and "," in suite and SEQUENCE["boundaries"](text) > boundaries:
                    boundaries, started = SEQUENCE["boundaries"](text), now
                result = DONE["completion"](text, mode, suite, only)
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
        elif mode in COMPLETING_MODES and progress.stalled():
            progress.record()
            return "stalled"
        elif (mode == "suite" or until_desktop_ready) and gate_timeout > 0 and now - started >= gate_timeout:
            expected = "desktop-ready" if until_desktop_ready else "suite DONE"
            print(f"==> No {expected} within {gate_timeout:g}s; stopping the VM", flush=True)
            return "deadline"
        elif not until_desktop_ready and idle_exit > 0 and now - last_output >= idle_exit:
            print(f"==> No serial output for {idle_exit:g}s; stopping the VM", flush=True)
            return "idle"
        time.sleep(0.2)
    serial_clock.observe()
    if until_desktop_ready:
        return "cancelled"
    if mode not in COMPLETING_MODES:
        return "exit"
    # An operator quit (monitor, window, or signal) must not reboot the guest.
    # A fatal guest record still identifies a guest death if QEMU exited first.
    text = serial.read_bytes().decode("utf-8", errors="replace") if serial.exists() else ""
    result = DONE["completion"](text, mode, suite, only) if mode in COMPLETING_MODES else None
    if process.returncode >= 0 and result and result[1].startswith("fatal kernel output:"):
        return "fatal"
    return "cancelled"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--serial", type=Path, required=True)
    parser.add_argument("--mode", required=True)
    parser.add_argument("--suite", default="")
    parser.add_argument("--case", default="", dest="only")
    parser.add_argument("--idle-exit", type=float, default=300)
    parser.add_argument("--gate-timeout", type=float, default=1800)
    parser.add_argument("--suite-hold", type=float, default=float(os.environ.get("BREENIX_SUITE_HOLD", "5")))
    parser.add_argument("--until-desktop-ready", action="store_true")
    parser.add_argument("--disk")
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    if args.idle_exit < 0 or args.gate_timeout <= 0 or args.suite_hold < 0:
        parser.error("idle exit and suite hold must be nonnegative; gate timeout must be positive")
    if args.until_desktop_ready and args.mode != "desktop":
        parser.error("--until-desktop-ready requires desktop mode")
    command = args.command[1:] if args.command[:1] == ["--"] else args.command
    if not command:
        parser.error("a QEMU command is required after --")

    # Cleanup is synchronous, including signals sent only to this watcher.
    def interrupted(signum, frame):
        raise SystemExit(128 + signum)

    for signum in (signal.SIGTERM, signal.SIGHUP, signal.SIGINT):
        signal.signal(signum, interrupted)
    clock = WallClockCheck(args.serial)
    serial_clock = SerialClock(args.serial)
    # qemu-arm64.py supplies this pidfile. A fresh-task QoS launch keeps the
    # supervisor's Popen PID while QEMU runs in a different process.
    pid_file = Path(str(args.serial) + ".qemu.pid")
    if "-pidfile" in command:
        pid_file = Path(command[command.index("-pidfile") + 1])
    pid_file.unlink(missing_ok=True)
    launch_env = os.environ.copy()
    if args.mode == "suite" and "window-manager" in args.suite.split(","):
        launch_env["BREENIX_QEMU_CASE_SAMPLE_SERIAL"] = str(args.serial.resolve())
    process = subprocess.Popen(command, env=launch_env)
    case_samples = None
    try:
        if args.mode == "suite" and "window-manager" in args.suite.split(","):
            case_samples = CaseSamples(process.pid, args.serial, pid_file=pid_file)
        reason = watch(process, args.serial, args.mode, args.suite, args.idle_exit,
                       args.gate_timeout, args.suite_hold, clock, args.only, args.until_desktop_ready, serial_clock, case_samples)
    finally:
        try:
            if case_samples:
                case_samples.close()
        finally:
            stop(process)
            serial_clock.observe()
    if args.until_desktop_ready:
        if reason != "done":
            print(f"FAIL: desktop did not become ready ({reason})", flush=True)
        return 0 if reason == "done" else 1
    if reason == "cancelled":
        print("==> QEMU exited; operator cancellation stops suite collection", flush=True)
        return 128 - process.returncode if process.returncode < 0 else 130
    clock_ok = True
    if args.mode in COMPLETING_MODES:
        clock_ok, clock_reason = clock.verdict()
        print(clock_reason, flush=True)
    if args.mode == "suite" and "," in args.suite:
        # Each suite's result, whether the sequence finished or the boot was stopped part way.
        ok, rows = SEQUENCE["score"](args.suite.split(","), args.serial, [args.serial], args.disk)
        for suite, status, detail in rows:
            print(f"{status} {suite}: {detail}", flush=True)
        passed = sum(status == "PASS" for _, status, _ in rows)
        print(f"{'PASS' if ok else 'FAIL'}: {passed} of {len(rows)} suites passed", flush=True)
        if reason in ("stalled", "deadline", "idle"):
            return 124
        return 0 if ok and clock_ok and reason == "done" else 1
    if reason == "stalled":
        return 124
    if reason == "fatal":
        return 1
    if args.mode in ("probe", "tests", "program"):
        serial = args.serial.read_bytes().decode("utf-8", errors="replace") if args.serial.exists() else ""
        result = DONE["completion"](serial, args.mode, args.suite, args.only)
        print(f"{'PASS' if result and result[0] else 'FAIL'}: {result[1] if result else 'no completion record'}", flush=True)
        return 0 if result and result[0] and clock_ok else 1
    if args.mode == "suite":
        serial = args.serial.read_bytes().decode("utf-8", errors="replace") if args.serial.exists() else ""
        result = DONE["completion"](serial, args.mode, args.suite, args.only)
        if result is None:
            print(f"FAIL: no complete SUITE {args.suite} DONE before {reason}", flush=True)
            return 1
        if not result[0]:
            print(f"FAIL: {result[1]}", flush=True)
            return 1
        manifest = json.loads((ROOT / f"docs/suites/{args.suite}.json").read_text())
        ok, detail = VERDICT["verdict"](manifest, args.serial, [args.serial], only=args.only, disk=args.disk)
        if ok and not args.only:
            ok, disk_detail = VERDICT["disk_verdict"](manifest.get("diskChecks", []), args.disk)
            if disk_detail:
                detail += f"; {disk_detail}"
        print(f"{'PASS' if ok else 'FAIL'}: {detail}", flush=True)
        return 0 if ok and clock_ok else 1
    if reason == "idle":
        return 0
    return process.returncode if process.returncode >= 0 else 128 - process.returncode


if __name__ == "__main__":
    raise SystemExit(main())
