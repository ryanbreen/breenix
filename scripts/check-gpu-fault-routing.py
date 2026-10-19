#!/usr/bin/env python3
"""Judge the stopped ARM64 gpu_fault_acceptance program boot."""

import argparse
from pathlib import Path
import re


def check(text):
    patterns = (
        r"^GPU_FAULT_TEST user syscall=500 handoff=1\r?$",
        r"^\[DATA_ABORT\] FAR=0x0*700000000000 .* from_el0=0\r?$",
        r"^GPU_FAULT_TEST recovered signal=11 kernel=running handoff=1\r?$",
        r"^\[GPU_FAULT_TEST kernel privilege=kernel handoff=1\]\r?$",
        r"^\[DATA_ABORT\] FAR=0x0*700000001000 .* from_el0=0\r?$",
        r"^\[GPU_PANIC_SCREEN shown=1 observed=1 pixels=4096000 mismatches=0\]\r?$",
    )
    records = [list(re.finditer(pattern, text, re.MULTILINE)) for pattern in patterns]
    if any(len(matches) != 1 for matches in records):
        return False, "missing or duplicate syscall fault, SIGSEGV recovery, kernel fault or screen"
    if len(re.findall(r"^\[DATA_ABORT\] FAR=", text, re.MULTILINE)) != 2:
        return False, "unexpected additional data abort"
    offsets = [matches[0].start() for matches in records]
    if offsets != sorted(offsets):
        return False, "fault, recovery and terminal-screen records are out of order"
    for record in (records[1][0], records[4][0]):
        esr = re.search(r"ESR=(0x[0-9a-fA-F]+)", record.group())
        if esr is None or (int(esr[1], 16) >> 26) & 0x3f != 0x25:
            return False, "fault was not a same-EL data abort"
    if re.search(r"unexpectedly|invalid kernel context|edge=(?:LOCKUP|PANIC)|kind=(?:LOCKUP|PANIC)|soft[ -]lockup", text, re.IGNORECASE):
        return False, "unexpected return, invalid context or soft lockup"
    if "deferring process cleanup" in text[offsets[4]:]:
        return False, "fatal kernel thread entered task recovery"
    return True, "syscall SIGSEGV reaped; kernel-thread EL1 abort terminal; 4096000 device pixels matched"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("serial", type=Path)
    args = parser.parse_args()
    try:
        ok, detail = check(args.serial.read_text(errors="replace"))
    except OSError as error:
        ok, detail = False, str(error)
    print(f"{'PASS' if ok else 'FAIL'}: {detail}")
    return 0 if ok else 1


if __name__ == "__main__":
    raise SystemExit(main())
