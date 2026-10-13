#!/usr/bin/env python3
"""Own a legacy boot process and stop it if test progress stalls for two minutes."""
import argparse
import os
from pathlib import Path
import runpy
import signal
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[1]
BootProgress = runpy.run_path(str(ROOT / 'scripts/boot_progress.py'))['BootProgress']
stop = runpy.run_path(str(ROOT / 'scripts/x86-gate-boot.py'))['stop']


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--worker', action='store_true', help=argparse.SUPPRESS)
    parser.add_argument('--serial', type=Path, action='append', required=True)
    parser.add_argument('--user-serial', type=Path)
    parser.add_argument('command', nargs=argparse.REMAINDER)
    args = parser.parse_args()
    command = args.command[1:] if args.command[:1] == ['--'] else args.command
    if not command:
        parser.error('a boot command is required after --')
    if not args.worker:
        # Retain a cleanup worker even if a caller uses Child.kill() (SIGKILL)
        # on the foreground handle. The worker owns and reaps the VM group.
        parent = os.getpid()
        worker = os.fork()
        if worker:
            for number in (signal.SIGTERM, signal.SIGINT, signal.SIGHUP):
                signal.signal(number, lambda number, frame: os.kill(worker, number))
            _, status = os.waitpid(worker, 0)
            return os.waitstatus_to_exitcode(status)
    else:
        parent = os.getppid()
    progress = BootProgress(args.serial[0], track_completion=True)
    child = subprocess.Popen(command, start_new_session=True)
    try:
        while child.poll() is None:
            if os.getppid() != parent:
                return 129
            progress.poll([(path, path == args.user_serial) for path in args.serial])
            if progress.completed:
                return 0
            if progress.stalled():
                progress.record()
                return 124
            time.sleep(.2)
        return child.returncode if child.returncode >= 0 else 128 - child.returncode
    finally:
        stop(child)


if __name__ == '__main__':
    for number in (signal.SIGTERM, signal.SIGINT, signal.SIGHUP):
        signal.signal(number, lambda number, frame: sys.exit(128 + number))
    sys.exit(main())
