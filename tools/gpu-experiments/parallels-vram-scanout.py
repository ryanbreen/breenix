#!/usr/bin/env python3
"""Black-box probe: does a virtio-gpu device's BAR0 memory keep what the CPU wrote across a 3D scanout?

    sudo python3 parallels-vram-scanout.py COMMAND [ARG...]

Run on a Linux guest whose virtio-gpu (1af4:1050) exposes memory in BAR0, as the firmware
framebuffer of a Parallels VM does. The probe writes a pattern into the first and last 4 KiB of
the first MiB of BAR0 through the PCI resource file, reads it back, runs COMMAND (a program that
puts a 3D resource on a scanout through KMS, for example scripts/parallels/egl_green_test.c),
and reads the pattern again. Nothing here touches the device's queues or registers.
"""
import glob
import mmap
import os
import subprocess
import sys
import time

SPAN = 1 << 20
PAGE = 4096


def device():
    for d in glob.glob("/sys/bus/pci/devices/*"):
        ids = [open(f"{d}/{n}").read().strip() for n in ("vendor", "device")]
        if ids == ["0x1af4", "0x1050"]:
            return d
    sys.exit("no virtio-gpu PCI device")


def main():
    d = device()
    fd = os.open(f"{d}/resource0", os.O_RDWR | os.O_SYNC)
    m = mmap.mmap(fd, SPAN)
    pattern = bytes((i * 7 + 3) & 0xFF for i in range(PAGE))
    regions = {"first": 0, "last": SPAN - PAGE}

    def report(when):
        kept = {name: m[at:at + PAGE] == pattern for name, at in regions.items()}
        print(f"{when}: " + " ".join(f"{n}={'kept' if k else 'changed'}" for n, k in kept.items())
              + f" first-16={m[0:16].hex()}")

    for at in regions.values():
        m[at:at + PAGE] = pattern
    report("written")
    time.sleep(1)
    report("after 1 s")
    run = subprocess.run(sys.argv[1:], capture_output=True, text=True)
    print(f"command exit {run.returncode}")
    report("after the 3D scanout")


if __name__ == "__main__":
    main()
