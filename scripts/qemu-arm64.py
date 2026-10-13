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
    has_cpu = False
    index = 0
    while index < len(arguments):
        option = arguments[index]
        if option in ("-accel", "-enable-kvm"):
            raise ValueError("select the accelerator with BREENIX_ARM64_QEMU_ACCEL")
        if option in ("-cpu", "-M", "-machine"):
            if index + 1 == len(arguments):
                raise ValueError(f"{option} needs a value")
            value = arguments[index + 1]
            if option == "-cpu":
                has_cpu = True
            else:
                if any(part.startswith("accel=") for part in value.split(",")):
                    raise ValueError("select the accelerator with BREENIX_ARM64_QEMU_ACCEL")
            if accelerator == "hvf":
                if option == "-cpu":
                    if value not in ("max", "host"):
                        raise ValueError(f"HVF cannot run CPU profile {value}; select TCG")
                    value = "host"
                else:
                    parts = value.split(",")
                    if parts[0] not in ("virt", "type=virt"):
                        raise ValueError("ARM64 HVF launcher requires the virt machine")
                    parts = [part for part in parts if not part.startswith("gic-version=")]
                    value = ",".join(parts + ["gic-version=3"])
            result.extend((option, value))
            index += 2
        else:
            result.append(option)
            index += 1
    if accelerator == "hvf" and not has_cpu:
        result.extend(("-cpu", "host"))
    return result


def main():
    arguments = sys.argv[1:]
    launcher = None
    print_profile = arguments[:1] == ["--print-profile"]
    if print_profile:
        arguments = arguments[1:]
    accelerator = os.environ.get("BREENIX_ARM64_QEMU_ACCEL")
    while arguments[:1] in (["--launcher"], ["--accel"]):
        if len(arguments) < 3:
            sys.exit(f"{arguments[0]} needs a value followed by the QEMU binary")
        if arguments[0] == "--accel":
            accelerator, arguments = arguments[1], arguments[2:]
        else:
            launcher, arguments = arguments[1], arguments[2:]
    if not arguments:
        sys.exit("usage: qemu-arm64.py [--launcher PROGRAM] QEMU [QEMU arguments...]")
    try:
        # A named CPU is a coverage requirement, not a request for the host CPU.
        if accelerator is None:
            cpus = [arguments[i + 1] for i, arg in enumerate(arguments[:-1]) if arg == "-cpu"]
            accelerator = "tcg" if any(cpu not in ("max", "host") for cpu in cpus) else default_accelerator()
        argv = command(arguments[0], arguments[1:], accelerator)
        cpu = argv[argv.index("-cpu") + 1] if "-cpu" in argv else "default"
        if print_profile:
            print(f"{accelerator}-{cpu}")
            return
        print(f"BREENIX_QEMU_CONFIG accel={accelerator} cpu={cpu}", file=sys.stderr, flush=True)
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
