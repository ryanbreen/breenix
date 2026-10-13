#!/usr/bin/env python3
"""Select the accelerator for an ARM64 QEMU launch, preserving TCG profiles."""

import os
import platform
from pathlib import Path
import subprocess
import sys


def default_accelerator(system=None, machine=None):
    system = system or platform.system()
    machine = machine or platform.machine()
    return "hvf" if system == "Darwin" and machine == "arm64" else "tcg"


def command(binary, arguments, accelerator, system=None, machine=None):
    system = system or platform.system()
    machine = machine or platform.machine()
    if accelerator not in ("tcg", "hvf"):
        raise ValueError("BREENIX_ARM64_QEMU_ACCEL must be tcg or hvf")
    if accelerator == "hvf" and (system != "Darwin" or machine != "arm64"):
        raise ValueError("ARM64 HVF requires an Apple Silicon macOS host")
    result = [binary, "-accel", accelerator]
    index = 0
    while index < len(arguments):
        option = arguments[index]
        if option in ("-accel", "-enable-kvm"):
            raise ValueError("select the accelerator with BREENIX_ARM64_QEMU_ACCEL")
        if option in ("-cpu", "-M", "-machine"):
            if index + 1 == len(arguments):
                raise ValueError(f"{option} needs a value")
            value = arguments[index + 1]
            if accelerator == "hvf":
                if option == "-cpu":
                    value = "host"
                else:
                    parts = value.split(",")
                    if parts[0] != "virt":
                        raise ValueError("ARM64 HVF launcher requires the virt machine")
                    parts = [part for part in parts if not part.startswith("gic-version=")]
                    value = ",".join(parts + ["gic-version=3"])
            result.extend((option, value))
            index += 2
        else:
            result.append(option)
            index += 1
    return result


def main():
    arguments = sys.argv[1:]
    launcher = None
    if arguments[:1] == ["--launcher"]:
        if len(arguments) < 3:
            sys.exit("--launcher needs a program followed by the QEMU binary")
        launcher, arguments = arguments[1], arguments[2:]
    if not arguments:
        sys.exit("usage: qemu-arm64.py [--launcher PROGRAM] QEMU [QEMU arguments...]")
    try:
        argv = command(arguments[0], arguments[1:],
                       os.environ.get("BREENIX_ARM64_QEMU_ACCEL", default_accelerator()))
        if launcher is None:
            launcher = os.environ.get("BREENIX_QEMU_UI_LAUNCHER")
        if not launcher:
            helper = Path(__file__).with_name("qemu-ui-launch.sh")
            launcher = subprocess.run([str(helper)], check=True, stdout=subprocess.PIPE,
                                      text=True).stdout.strip()
        if not launcher:
            raise ValueError("QEMU QoS helper returned an empty launcher")
    except ValueError as error:
        sys.exit(str(error))
    # The QoS library removes its injection environment when it initializes.
    # Run its launcher directly before QEMU, after this Python process.
    argv.insert(0, launcher)
    os.execvp(argv[0], argv)


if __name__ == "__main__":
    main()
