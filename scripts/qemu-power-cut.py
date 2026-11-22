#!/usr/bin/env python3
"""Cut an ARM64 QEMU BreenFS writer, score its stopped disk, and boot recovery."""

import argparse
from contextlib import contextmanager
import json
import os
from pathlib import Path
import runpy
import signal
import subprocess
import sys
import tempfile
import time

from metal import breenfs_replay, metal_power_cut

HERE = Path(__file__).resolve().parent
completion = runpy.run_path(str(HERE / "wait-boot-done.py"))["completion"]
WRITER = "power-cut/writer"
RECOVERY = "power-cut/recovery"
READY = b"SUITE_SEQUENCE READY filesystem"


def write_json(path, data):
    with path.open("x") as output:
        json.dump(data, output, indent=2)
        output.write("\n")


@contextmanager
def boot(args, serial, case, extra, *, hard_cut=False):
    command = args.command + [
        "-fw_cfg", f"name=opt/breenix/case,string={case}",
        "-device", "virtio-blk-device,drive=ext2",
        # cache=unsafe ignores guest flushes. Keep QEMU honoring them even
        # though the host page cache itself survives the simulated outage.
        "-drive", f"if=none,id=ext2,format=raw,file={str(args.disk).replace(',', ',,')},cache=writeback,aio=threads",
        "-chardev", f"stdio,id=con,mux=on,signal=off,logfile={str(serial).replace(',', ',,')}",
        "-serial", "chardev:con", "-mon", "chardev=con,mode=readline",
        *extra,
    ]
    serial.touch(exist_ok=False)
    boot_id = ""
    status = 1
    if args.profile:
        def slot(action, *values):
            subprocess.run([sys.executable, HERE / "host-slots.py", action, *values], check=True)

        slot("serial", serial)
        slot("quiet", HERE.parent / "docs/suites/filesystem.json")
        env = dict(os.environ, BREENIX_VIGIL_DISPLAY="false", BREENIX_VIGIL_ONLY_CASE=case)
        boot_id = subprocess.check_output([
            HERE / "vigil-record.sh", "start", "qemu", "suite", "filesystem", serial,
            "", args.profile,
        ], env=env, text=True).strip()
    print(f"==> Booting {case} (serial: {serial}; Vigil: {boot_id or 'unavailable'})", flush=True)
    try:
        if args.profile and os.getloadavg()[0] > 8:
            raise ValueError("ARM64 power-cut boots require one-minute host load at or below 8")
        env = dict(os.environ, BREENIX_QEMU_HARD_CUT="1" if hard_cut else "0")
        with subprocess.Popen(command, env=env) as process:
            try:
                yield process, boot_id
                status = 0
            finally:
                if process.poll() is None:
                    process.terminate()
                    try:
                        process.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        process.kill()
                        process.wait()
    finally:
        if args.profile:
            slot("header", serial)
            subprocess.run([HERE / "vigil-record.sh", "finish", boot_id, str(status)], check=True)


def fail_on_guest_error(data, case):
    result = completion(data.decode(errors="replace"), "suite", "filesystem", case)
    if result and not result[0]:
        raise ValueError(result[1])
    if any(line.startswith(b"SUITE filesystem ERROR ") for line in data.splitlines()):
        raise ValueError(f"{case} reported a guest error")


def kill_qemu(process, pidfile):
    pid = int(pidfile.read_text())
    if process.poll() is not None:
        raise ValueError("QEMU exited before the cut trigger")
    if pid == process.pid:
        process.kill()
    else:
        # On a clamped Mac the launched child is qemu-unclamp, which asks
        # its launchd worker to signal the QEMU subprocess it owns.
        process.send_signal(signal.SIGUSR1)
    return pid


def wait_for_cut(args, process, pidfile):
    deadline = time.monotonic() + args.gate_timeout
    timer_started = None
    writer = None
    acknowledged = False
    marker = f"[breenfs] commit-window {args.power_cut_window}".encode()
    with args.serial.open("rb") as log:
        while process.poll() is None:
            now = time.monotonic()
            if timer_started is None and pidfile.is_file():
                timer_started = now
            if args.power_cut_after is not None and timer_started is not None:
                if now >= timer_started + args.power_cut_after:
                    return dict(timerStartedAt=timer_started), writer
            if now >= deadline:
                raise ValueError("QEMU did not reach the power-cut trigger before the deadline")
            offset = log.tell()
            line = log.readline()
            if not line.endswith(b"\n"):
                log.seek(offset)
                time.sleep(0.01)
                continue
            line = line.rstrip(b"\r\n")
            fail_on_guest_error(line + b"\n", WRITER)
            if line.startswith(b"SUITE filesystem DONE "):
                raise ValueError("the writer finished before the cut trigger")
            if match := breenfs_replay.WRITER_START.fullmatch(line):
                writer = dict(event="power-cut-writer-start", t=now, nonce=match[1].decode())
            elif writer and (ack := breenfs_replay.ACK.fullmatch(line)):
                if ack[1] == b"1" and ack[3].decode() == writer["nonce"]:
                    acknowledged = True
            elif acknowledged and line == marker and args.power_cut_window:
                return dict(window=args.power_cut_window, triggerLine=marker.decode(),
                            triggerSerialOffset=offset, triggerObservedAt=now), writer
    raise ValueError("QEMU exited before the cut trigger")


def cold_check(args, mode):
    result = subprocess.run([
        sys.executable, HERE / "metal/breenfs_replay.py", mode, args.serial, args.disk,
        "--tools-dir", args.tools_dir, "--json",
    ], capture_output=True, text=True, timeout=60)
    if result.returncode:
        detail = result.stderr.strip()
        raise ValueError(detail.removeprefix("FAIL: "))
    return json.loads(result.stdout)


def wait_for_diagnostic(process, serial, case, timeout):
    expected = f"SUITE filesystem DONE passed=1 failed=0 skipped=0 total=1 only={case}".encode()
    deadline = time.monotonic() + timeout
    while process.poll() is None and time.monotonic() < deadline:
        data = serial.read_bytes()
        fail_on_guest_error(data, case)
        lines = data.replace(b"\r\n", b"\n").split(b"\n")[:-1]
        if expected in lines and READY in lines and lines.index(expected) < lines.index(READY):
            return
        time.sleep(0.05)
    raise ValueError(f"{case} did not finish with its diagnostic receipt")


def score_recovery(serial, cold):
    data = serial.read_bytes()
    fail_on_guest_error(data, RECOVERY)
    lines = data.splitlines()
    receipt = (f"BREENFS POWER-CUT RECOVERY root-is-breenfs bytes={cold['logBytes']} "
               f"sha256={cold['logSha256']}").encode()
    if [line for line in lines if line.startswith(b"BREENFS POWER-CUT RECOVERY ")] != [receipt]:
        raise ValueError("the mounted crash log differs from cold replay")
    if missing := cold.get("missingRingB"):
        repair = (f"[breenfs] recovered-superblock ring=B slot={missing['slot']} "
                  f"generation={missing['generation']}").encode()
        if lines[:lines.index(receipt)].count(repair) != 1:
            raise ValueError("writable recovery did not report repairing the expected ring B slot")


def run(args):
    directory = args.serial.parent
    # The launcher stages a raw BreenFS image in a new private directory.
    # Bind both QEMU invocations and the replay input to that exact inode.
    image = args.disk.stat()
    if args.disk.is_symlink() or not args.disk.is_file() or image.st_nlink != 1:
        raise ValueError("the disposable root must be a private regular image")

    def check_disk():
        if not os.path.samestat(image, args.disk.stat()):
            raise ValueError("the disposable root image was replaced")

    extra = []
    if fault := metal_power_cut.kernel_fault(args):
        _, cmdline = fault
        if cmdline:
            intent = directory / "writer.intent"
            with intent.open("x") as file:
                file.write(f"BREENIX-BOOT 2\ntarget suite filesystem case {WRITER}\ncmdline {cmdline}\n")
            extra = ["-fw_cfg", f"name=opt/breenix/intent,file={str(intent).replace(',', ',,')}"]

    if args.breenfs_bit_flip:
        with boot(args, args.serial, WRITER, extra) as (process, boot_id):
            wait_for_diagnostic(process, args.serial, WRITER, args.gate_timeout)
        check_disk()
        fail_on_guest_error(args.serial.read_bytes(), WRITER)
        cold = cold_check(args, "bit-flip")
        write_json(directory / "bit-flip-readback.json", dict(cold, passed=True, boot=boot_id))
        print(f"PASS: block {cold['block']} returned ReadError::Data and failed C1", flush=True)
        return

    with tempfile.TemporaryDirectory(prefix="breenix-cut-", dir="/tmp") as temporary:
        pidfile = Path(temporary) / "qemu.pid"
        attached_at = time.monotonic()
        with boot(args, args.serial, WRITER, [*extra, "-pidfile", str(pidfile)],
                  hard_cut=True) as (process, boot_id):
            cut, writer = wait_for_cut(args, process, pidfile)
            cut_at = time.monotonic()
            pid = kill_qemu(process, pidfile)
            qemu_exit = process.wait(timeout=10)
            if qemu_exit != -signal.SIGKILL:
                raise ValueError(f"QEMU did not exit from SIGKILL (status {qemu_exit})")
            stopped_at = time.monotonic()
    check_disk()
    cut.update(cutAt=cut_at, powerOffAt=stopped_at, powerOffConfirmed=True,
               qemuPid=pid, qemuExit=qemu_exit, after=args.power_cut_after)
    run_id = boot_id or directory.name
    # Supply the replay checker's evidence envelope with the VM equivalents:
    # the freshly staged, attached root and the reaped SIGKILL exit.
    events = [dict(event="disk-root-record", t=attached_at, matchesLastInstall=True, installedBy=run_id)]
    if writer:
        events.append(writer)
    events.append(dict(event="power-off", t=stopped_at, off=True))
    write_json(directory / "run.json", dict(id=run_id, mode="suite", disk="root", rootFs="breenfs",
                                           onlyCase=WRITER, image=str(args.disk), powerCut=cut))
    (directory / "events.jsonl").write_text("".join(json.dumps(event) + "\n" for event in events))
    data = args.serial.read_bytes()
    fail_on_guest_error(data, WRITER)
    if any(line.startswith(b"SUITE filesystem DONE ") for line in data.splitlines()):
        raise ValueError("the writer finished before QEMU stopped")
    cold = cold_check(args, "replay")
    write_json(directory / "cut-readback.json", dict(cold, passed=True))
    print(f"PASS: cold replay, {cold['acknowledged']} acknowledged, {cold['coldState']}", flush=True)
    recovery_serial = directory / "recovery.txt"
    check_disk()
    with boot(args, recovery_serial, RECOVERY, []) as (process, recovery_id):
        wait_for_diagnostic(process, recovery_serial, RECOVERY, args.gate_timeout)
    check_disk()
    score_recovery(recovery_serial, cold)
    write_json(directory / "cut-cycle.json", dict(writer=boot_id, recovery=recovery_id, passed=True))
    print("PASS: no-restage recovery read the cold replay log", flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    metal_power_cut.add_options(parser)
    parser.add_argument("--serial", required=True, type=Path)
    parser.add_argument("--disk", required=True, type=Path)
    parser.add_argument("--tools-dir", required=True, type=Path)
    parser.add_argument("--gate-timeout", required=True, type=metal_power_cut.positive_seconds)
    parser.add_argument("--profile")
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    if args.command[:1] == ["--"]:
        args.command.pop(0)
    if not args.command or not (metal_power_cut.options(args) or args.breenfs_bit_flip):
        parser.error("a power-cut or bit-flip option and a QEMU command are required")
    if args.power_cut_after is not None and args.power_cut_after >= args.gate_timeout:
        parser.error("--power-cut-after must be less than --gate-timeout")
    args.case = WRITER
    args.serial = args.serial.absolute()
    args.disk = args.disk.absolute()
    if (args.disk.parent != args.serial.parent or args.disk.name != "breenfs-writable.img"
            or args.serial.name in ("recovery.txt", "run.json", "events.jsonl", "cut-readback.json",
                                    "cut-cycle.json", "bit-flip-readback.json", "writer.intent")):
        parser.error("the writer serial and disposable breenfs-writable.img must share their run directory")

    def interrupted(signum, frame):
        raise SystemExit(128 + signum)

    for signum in (signal.SIGTERM, signal.SIGHUP, signal.SIGINT):
        signal.signal(signum, interrupted)
    try:
        run(args)
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        print(f"FAIL: {error}", file=sys.stderr, flush=True)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
