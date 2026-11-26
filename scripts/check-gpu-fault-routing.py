#!/usr/bin/env python3
"""Judge the stopped ARM64 gpu_fault_acceptance program boot.

Two writers reach serial. The kernel prints its fault records and test lines
synchronously. The program's stdout is a pipe that the probe polls and relays,
so a program line lands at some point after its write, never before it.
The checker therefore orders only what the code orders: kernel records among
themselves, relayed lines among themselves (one pipe, read in order), and a
relayed line after a kernel record that the program waited on before writing
it. Each fault is identified by its own record, not by where the marker lands.
"""

import argparse
from pathlib import Path
import re

MARKER = r"^GPU_FAULT_TEST user syscall=0x7fff0000 handoff=1\r?$"
RECOVERED = r"^GPU_FAULT_TEST recovered signal=11 kernel=running handoff=1\r?$"
SYSCALL_ABORT = r"^\[DATA_ABORT\] FAR=0x0*700000000000 .* from_el0=0\r?$"
PRIVILEGE = r"^\[GPU_FAULT_TEST kernel privilege=kernel handoff=1\]\r?$"
KERNEL_ABORT = r"^\[DATA_ABORT\] FAR=0x0*700000001000 .* from_el0=0\r?$"
SCREEN = r"^\[GPU_PANIC_SCREEN shown=1 observed=1 pixels=4096000 mismatches=0\]\r?$"
ABORT_HEADER = r"^\[DATA_ABORT\] FAR="
# The fork child's main thread is named after its process (process/manager.rs).
PROGRAM_THREAD = "/usr/bin/gpu_fault_acceptance_main"
KERNEL_THREAD = "gpu-fault-test"


def abort_record(text, header, end):
    """Return (same-EL, is-write, tid, thread name) from one abort's dump."""
    esr = re.search(r"ESR=(0x[0-9a-fA-F]+)", header.group())
    if esr is None:
        return None
    esr = int(esr[1], 16)
    dump = text[header.end():end]
    postmortem = re.search(r"^\[FATAL_POSTMORTEM\]", dump, re.MULTILINE)
    if postmortem is not None:
        dump = dump[:postmortem.start()]
    threads = re.findall(r"\btid=(\d+) name=(\S+)\r?$", dump, re.MULTILINE)
    if len(threads) != 1:
        return None
    # ESR EC 0x25 is a data abort taken without an EL change; ISS bit 6 is WnR.
    return (esr >> 26) & 0x3f == 0x25, bool(esr & (1 << 6)), int(threads[0][0]), threads[0][1]


def check(text):
    patterns = (MARKER, SYSCALL_ABORT, RECOVERED, PRIVILEGE, KERNEL_ABORT, SCREEN)
    records = [list(re.finditer(pattern, text, re.MULTILINE)) for pattern in patterns]
    if any(len(matches) != 1 for matches in records):
        return False, "missing or duplicate syscall fault, SIGSEGV recovery, kernel fault or screen"
    aborts = list(re.finditer(ABORT_HEADER, text, re.MULTILINE))
    if len(aborts) != 2:
        return False, "unexpected additional data abort"
    marker, syscall_abort, recovered, privilege, kernel_abort, screen = (
        matches[0] for matches in records)
    if [syscall_abort.start(), kernel_abort.start()] != [abort.start() for abort in aborts]:
        return False, "the first data abort is not the op 2 syscall fault"
    # Kernel records follow the program: op 2 faults, the parent reaps the child,
    # op 1 starts the kernel thread, which announces itself and faults.
    kernel_order = [syscall_abort.start(), privilege.start(), kernel_abort.start(), screen.start()]
    if kernel_order != sorted(kernel_order):
        return False, "kernel fault, kernel-thread and terminal-screen records are out of order"
    # One pipe: the child's marker precedes the parent's post-reap line, which
    # the parent writes only after the op 2 fault killed the child. The screen
    # record is printed after the terminal latch, which parks a peer CPU at its
    # next timer tick (kernel/src/gpu/panic.rs stop_if_terminal).
    if not marker.start() < recovered.start() < screen.start():
        return False, "program marker and SIGSEGV recovery are out of pipe order"
    if recovered.start() < syscall_abort.start():
        return False, "SIGSEGV recovery reported before the syscall fault"
    syscall_fault = abort_record(text, syscall_abort, kernel_abort.start())
    kernel_fault = abort_record(text, kernel_abort, len(text))
    if syscall_fault is None or kernel_fault is None:
        return False, "fault record lacks one faulting thread or its syndrome"
    if not (syscall_fault[0] and kernel_fault[0]):
        return False, "fault was not a same-EL data abort"
    # Op 2 stores to 0x7000_0000_0000 in the program's syscall; the kernel
    # thread loads from 0x7000_0000_1000 (kernel/src/gpu_fault_acceptance.rs).
    if not syscall_fault[1] or syscall_fault[3] != PROGRAM_THREAD:
        return False, "first data abort is not the program's op 2 store"
    if kernel_fault[1] or kernel_fault[3] != KERNEL_THREAD:
        return False, "second data abort is not the kernel thread's load"
    deferred = re.findall(r"^\[DATA_ABORT\] deferred_tid=(\d+) queued=1\r?$",
                          text[syscall_abort.end():privilege.start()], re.MULTILINE)
    if deferred != [str(syscall_fault[2])]:
        return False, "SIGSEGV exit was not queued for the faulting program thread"
    if re.search(r"unexpectedly|invalid kernel context|edge=(?:LOCKUP|PANIC)|kind=(?:LOCKUP|PANIC)|soft[ -]lockup", text, re.IGNORECASE):
        return False, "unexpected return, invalid context or soft lockup"
    if "deferring process cleanup" in text[kernel_abort.start():]:
        return False, "fatal kernel thread entered task recovery"
    return True, "syscall SIGSEGV reaped; kernel-thread EL1 abort terminal; 4096000 device pixels matched"


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
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
