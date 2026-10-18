#!/usr/bin/env python3
"""Parallels host renderer attestation by host GPU time (docs/gui/gpu.md section 1.6.5).

    scripts/parallels/gpu-time.py sample --vm NAME --serial LOG --out RECORD
    scripts/parallels/gpu-time.py judge RECORD

Nothing in virtio-gpu lets the guest tell a GPU from a software renderer, so on Parallels the
claim rests on the host: macOS's accelerator registry keeps, for each GPU user client, the GPU
time its submissions used (`AppUsage`/`accumulatedGPUTime`, nanoseconds), and names the
process that opened the client (`IOUserClientCreator`). Every GPU client a Parallels VM opens
belongs to its VM process.

`sample` records the VM's configuration, then until the VM process exits appends to RECORD
(JSON lines) the VM process's accumulated GPU time and, with the host time each was seen,
the gpu suite's records and its attestation markers from the serial log. A user client's
time counts from the first sample that shows it, and time a client accumulated before the
VM process opened it cannot exist, so a client first seen with time is counted whole.

`judge` accepts the run when the VM has 3D acceleration on and the `performance` category's
GPU time exceeds the idle window's, scaled to the category's length, by at least 10% of the
category's length. The idle window is the suite's `SUITE_IDLE gpu performance` interval
just before `performance` (libbreenix suite runner, `Category::idle_before`). The category runs
from the end of that window to its last CASE record. Exit 0 accepts and prints the figures;
exit 1 refuses and names why.
"""
import argparse
import json
import re
import subprocess
import sys
import time

INTERVAL = 0.25
FLOOR = 0.10
CLIENT = re.compile(r"^\s*[| ]*\+-o (\S+)\s+<class [^,]+, id (0x[0-9a-f]+)")
CREATOR = re.compile(r'"IOUserClientCreator" = "pid (\d+),')
USAGE = re.compile(r'"accumulatedGPUTime"=(\d+)')
WANTED = re.compile(r"^(SUITE gpu (?:START|CASE|DONE)|SUITE_IDLE gpu performance) ")


def run(*cmd):
    return subprocess.run(cmd, capture_output=True, text=True, check=False).stdout


def vm_info(vm):
    return run("prlctl", "list", "-i", vm)


def vm_pid(uuid):
    """The VM process: Parallels' per-VM process whose arguments name the VM's UUID."""
    for line in run("ps", "-axo", "pid=,command=").splitlines():
        pid, _, command = line.strip().partition(" ")
        if "prl_vm_app" in command and uuid in command:
            return int(pid)
    return None


def client_times(pid):
    """Accumulated GPU nanoseconds of each GPU user client process `pid` opened, by registry id."""
    times, current = {}, None
    for line in run("ioreg", "-l", "-w0", "-r", "-c", "IOAccelerator").splitlines():
        match = CLIENT.match(line)
        if match:
            current = {"id": match.group(2), "pid": None, "ns": 0}
            continue
        if current is None:
            continue
        creator = CREATOR.search(line)
        if creator:
            current["pid"] = int(creator.group(1))
        if '"AppUsage"' in line:
            current["ns"] = sum(int(v) for v in USAGE.findall(line))
        if current["pid"] == pid:
            times[current["id"]] = current["ns"]
    return times


def sample(args):
    info = vm_info(args.vm)
    uuid = re.search(r"^ID: \{?([0-9a-f-]+)\}?", info, re.MULTILINE)
    video = re.search(r"^\s*video .*$", info, re.MULTILINE)
    accel = re.search(r"\b3d-acceleration=(\S+)", video.group(0)) if video else None
    with open(args.out, "w", encoding="utf-8") as out:
        def write(record):
            out.write(json.dumps(record) + "\n")
            out.flush()
        pid = None
        deadline = time.monotonic() + 60
        while uuid and pid is None and time.monotonic() < deadline:
            pid = vm_pid(uuid.group(1))
            if pid is None:
                time.sleep(INTERVAL)
        write({"kind": "vm", "vm": args.vm, "pid": pid,
               "video": video.group(0).strip() if video else None,
               "accel": accel.group(1) if accel else None})
        if pid is None:
            return 1
        seen, total, offset, partial = {}, 0, 0, b""
        while True:
            alive = subprocess.run(["kill", "-0", str(pid)], capture_output=True).returncode == 0
            now = time.monotonic()
            for client, ns in client_times(pid).items():
                total += max(0, ns - seen.get(client, 0))
                seen[client] = ns
            write({"kind": "gpu", "t": now, "ns": total, "clients": len(seen)})
            try:
                with open(args.serial, "rb") as serial:
                    serial.seek(offset)
                    data = serial.read()
            except FileNotFoundError:
                data = b""
            offset += len(data)
            lines = (partial + data).split(b"\n")
            partial = lines.pop()
            for raw in lines:
                line = raw.decode("utf-8", "replace").strip()
                if WANTED.match(line):
                    write({"kind": "line", "t": now, "line": line})
            if not alive:
                return 0
            time.sleep(INTERVAL)


def gpu_at(samples, t):
    """Accumulated GPU nanoseconds at host time `t`, interpolated between samples."""
    for (t0, n0), (t1, n1) in zip(samples, samples[1:]):
        if t0 <= t <= t1:
            return n0 + (n1 - n0) * (t - t0) / (t1 - t0) if t1 > t0 else n1
    return None


def judge_record(path, wait):
    deadline = time.monotonic() + wait
    while True:
        records = []
        try:
            with open(path, encoding="utf-8") as f:
                records = [json.loads(line) for line in f if line.strip()]
        except (OSError, ValueError) as e:
            return f"unreadable GPU time record {path}: {e}"
        lines = [(r["t"], r["line"]) for r in records if r.get("kind") == "line"]
        samples = [(r["t"], r["ns"]) for r in records if r.get("kind") == "gpu"]
        last_line = max((t for t, _ in lines), default=None)
        # The sample after the last record bounds its time; wait briefly for the sampler.
        if last_line is None or (samples and samples[-1][0] > last_line) or time.monotonic() > deadline:
            break
        time.sleep(INTERVAL)
    vm = next((r for r in records if r.get("kind") == "vm"), None)
    if vm is None or vm.get("pid") is None:
        return "the sampler found no VM process"
    if vm.get("accel") is None or vm["accel"].lower() in ("off", "0", "no", "disabled"):
        return f"the VM's 3D acceleration is {vm.get('accel')!r}, not on"
    begin = next((t for t, l in lines if l.startswith("SUITE_IDLE gpu performance begin")), None)
    end = next((t for t, l in lines if l.startswith("SUITE_IDLE gpu performance end")), None)
    perf = [t for t, l in lines if l.startswith("SUITE gpu CASE performance/")]
    if begin is None or end is None or end <= begin:
        return "the serial carries no idle window before the performance category"
    if not perf:
        return "the serial carries no performance CASE record"
    stop = perf[-1]
    idle_gpu = [gpu_at(samples, begin), gpu_at(samples, end)]
    cat_gpu = [gpu_at(samples, end), gpu_at(samples, stop)]
    if None in idle_gpu or None in cat_gpu:
        return "the GPU time samples do not cover the idle window and the performance category"
    idle_s, cat_s = end - begin, stop - end
    idle_ns, cat_ns = idle_gpu[1] - idle_gpu[0], cat_gpu[1] - cat_gpu[0]
    expected_idle = idle_ns / idle_s * cat_s
    excess = cat_ns - expected_idle
    figures = (f"idle {idle_ns / 1e6:.1f} ms GPU over {idle_s:.2f} s; performance {cat_ns / 1e6:.1f} ms GPU over "
               f"{cat_s:.2f} s; excess {excess / 1e6:.1f} ms, floor {FLOOR * cat_s * 1e3:.1f} ms")
    if excess < FLOOR * cat_s * 1e9:
        return f"host GPU time does not show the performance category's work: {figures}"
    print(f"Parallels host GPU time accepted (pid {vm['pid']}, 3D {vm['accel']}): {figures}")
    return None


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="command", required=True)
    s = sub.add_parser("sample")
    s.add_argument("--vm", required=True)
    s.add_argument("--serial", required=True)
    s.add_argument("--out", required=True)
    j = sub.add_parser("judge")
    j.add_argument("record")
    j.add_argument("--wait", type=float, default=5.0)
    args = parser.parse_args()
    if args.command == "sample":
        return sample(args)
    refusal = judge_record(args.record, args.wait)
    if refusal:
        print(f"Parallels host GPU time refused: {refusal}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
