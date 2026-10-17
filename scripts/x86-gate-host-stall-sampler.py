#!/usr/bin/env python3
"""Record how long the host keeps each vCPU of one x86 gate boot from running.

Usage: x86-gate-host-stall-sampler.py OUTDIR [PERIOD_SECONDS]

Started by docker/qemu/run-x86-gate.sh before a boot and stopped after it.
It writes OUTDIR/host-stall.jsonl. For each QEMU the boot starts (found by its
serial log, OUTDIR/serial_kernel.log), until that QEMU exits:

* a `guest` line: the QEMU pid, each vCPU thread by its KVM vCPU id (the
  guest's APIC id), and KVM's TSC offset for each vCPU, so a guest TSC value
  can be placed among the samples; it is written again if they change;
* then one sample line per period: host wall time, the guest's TSC at that
  moment (this host's TSC plus vCPU 0's offset), each host CPU's cumulative
  steal (in clock ticks, `clk_tck` per second), and per vCPU its cumulative
  run delay and run time (ns), the host CPU it last ran on and its state.

The guest cannot write any of this. scripts/score-sigkill-teardown.py reads it
to judge a SIGKILL kick window the guest reports as starved: the window must
show the host keeping its vCPUs from running. Read-only: it reads /proc,
KVM's debugfs and this CPU's TSC.
"""
import ctypes
import glob
import json
import mmap
import os
import pathlib
import signal
import sys
import time


def rdtsc_function():
    """This CPU's TSC, read by a five-instruction stub."""
    # rdtsc; shl rdx, 32; or rax, rdx; ret
    code = bytes([0x0F, 0x31, 0x48, 0xC1, 0xE2, 0x20, 0x48, 0x09, 0xD0, 0xC3])
    page = mmap.mmap(-1, mmap.PAGESIZE, prot=mmap.PROT_READ | mmap.PROT_WRITE | mmap.PROT_EXEC)
    page.write(code)
    address = ctypes.addressof(ctypes.c_char.from_buffer(page))
    function = ctypes.CFUNCTYPE(ctypes.c_uint64)(address)
    function.page = page
    return function


def names_serial(pid, serial):
    """Whether process `pid` was given `serial` as a `-serial file:` log. The
    gate's booters take the file and hand QEMU a relay socket instead, so the
    path is on the booter above QEMU (scripts/x86_serial_relay.py)."""
    try:
        args = pathlib.Path(f"/proc/{pid}/cmdline").read_bytes().split(b"\0")
        cwd = os.readlink(f"/proc/{pid}/cwd")
    except OSError:
        return False
    return any(arg.startswith(b"file:")
               and os.path.realpath(os.path.join(cwd, arg[5:].decode(errors="replace"))) == serial
               for arg in args)


def parent(pid):
    try:
        status = pathlib.Path(f"/proc/{pid}/status").read_text()
    except OSError:
        return None
    ppid = next((line.split()[1] for line in status.splitlines() if line.startswith("PPid:")), "0")
    return None if ppid in ("0", "1") else ppid


def guest_qemu(serial, seen):
    """A QEMU this boot started that has not been recorded yet."""
    for comm in glob.glob("/proc/[0-9]*/comm"):
        pid = comm.split("/")[2]
        try:
            if pid in seen or not pathlib.Path(comm).read_text().startswith("qemu-system-x86"):
                continue
        except OSError:
            continue
        ancestor = pid
        for _ in range(5):
            if ancestor is None:
                break
            if names_serial(ancestor, serial):
                return pid
            ancestor = parent(ancestor)
    return None


def vcpu_threads(pid):
    threads = {}
    for comm in glob.glob(f"/proc/{pid}/task/*/comm"):
        try:
            name = pathlib.Path(comm).read_text().strip()
        except OSError:
            continue
        if name.startswith("CPU ") and name.endswith("/KVM"):
            threads[name[4:-4]] = comm.split("/")[4]
    return threads


def tsc_offsets(pid):
    """KVM's TSC offset and scaling ratio for each vCPU of `pid`."""
    offsets, scaling = {}, {}
    for directory in glob.glob(f"/sys/kernel/debug/kvm/{pid}-*/vcpu*"):
        vcpu = directory.rsplit("vcpu", 1)[1]
        try:
            offsets[vcpu] = int(pathlib.Path(directory, "tsc-offset").read_text())
        except (OSError, ValueError):
            continue
        try:
            scaling[vcpu] = pathlib.Path(directory, "tsc-scaling-ratio").read_text().strip()
        except OSError:
            pass
    return offsets, scaling


def steal():
    with open("/proc/stat") as stat:
        return [int(line.split()[8]) for line in stat if line.startswith("cpu") and line[3].isdigit()]


def thread_sample(pid, tid):
    base = f"/proc/{pid}/task/{tid}"
    run, delay, _ = pathlib.Path(base, "schedstat").read_text().split()
    fields = pathlib.Path(base, "stat").read_text().rsplit(")", 1)[1].split()
    return [int(delay), int(run), int(fields[36]), fields[0]]


def sample_guest(out, pid, period, rdtsc, stopping):
    guest, checked = None, 0.0
    while not stopping and os.path.exists(f"/proc/{pid}"):
        began = time.time()
        # Threads and offsets settle once QEMU has created its vCPUs; a guest
        # that writes its TSC changes an offset, so they are read again.
        if guest is None or began - checked >= 1.0:
            checked = began
            threads = vcpu_threads(pid)
            offsets, scaling = tsc_offsets(pid)
            current = {"kind": "guest", "pid": int(pid), "vcpus": threads,
                       "tsc_offsets": offsets, "tsc_scaling": scaling}
            if current != guest and threads and set(offsets) >= set(threads):
                out.write(json.dumps(current) + "\n")
                guest = current
        if guest is None:
            time.sleep(0.1)
            continue
        threads, offsets = guest["vcpus"], guest["tsc_offsets"]
        # TSC offsets are signed; the guest's TSC wraps as a u64.
        record = {"t": round(began, 4), "tsc": (rdtsc() + offsets.get("0", 0)) % (1 << 64), "steal": steal()}
        vcpus = {}
        for vcpu, tid in threads.items():
            try:
                vcpus[vcpu] = thread_sample(pid, tid)
            except (OSError, ValueError, IndexError):
                continue
        record["vcpu"] = vcpus
        out.write(json.dumps(record, separators=(",", ":")) + "\n")
        out.flush()
        time.sleep(max(0.0, period - (time.time() - began)))


def main():
    outdir = os.path.realpath(sys.argv[1])
    period = float(sys.argv[2]) if len(sys.argv) > 2 else 0.02
    serial = os.path.join(outdir, "serial_kernel.log")
    stopping = []
    signal.signal(signal.SIGTERM, lambda *_: stopping.append(True))
    signal.signal(signal.SIGINT, lambda *_: stopping.append(True))
    rdtsc = rdtsc_function()
    seen = set()
    with open(os.path.join(outdir, "host-stall.jsonl"), "w") as out:
        out.write(json.dumps({"kind": "start", "clk_tck": os.sysconf("SC_CLK_TCK"), "period": period}) + "\n")
        out.flush()
        while not stopping:
            pid = guest_qemu(serial, seen)
            if pid is None:
                time.sleep(0.2)
                continue
            seen.add(pid)
            sample_guest(out, pid, period, rdtsc, stopping)
        out.write(json.dumps({"kind": "end", "t": round(time.time(), 4)}) + "\n")


if __name__ == "__main__":
    main()
