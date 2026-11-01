#!/usr/bin/env python3
"""Watch one lane-owned C5c boot from the boot host, using its deployed tools.

Source host.env before invoking this file with BOOT_ID COMMIT. Only the capture
child's explicit stop-request exit status authorizes metal-run stop. A broken
watcher is reported to its caller; it is never evidence of a hardware failure.
"""
import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import time

STOP_REQUESTED = 10


def log(message):
    print(message, flush=True)


def stop_reason(line, report=log):
    """Parse only records used for a C5c stop decision; keep errors nonfatal."""
    if line == 'GT710_C5C_DONE':
        return 'C5c-complete'
    if re.search(r'^GT710_.*(?:ERROR|NATIVE_FAULT|failed:|REFUSAL)|^LOCKUP|^PANIC|^panicked at', line):
        return 'gpu-fault-or-failed-step'
    if (line.startswith('GT710_PMU_WAIT ') and
            ('name=send ' in line or 'name=recv ' in line) and 'matched=false' in line):
        return 'PMU-ring-timeout'
    try:
        if line.startswith('GT710_TEMPERATURE {'):
            row = json.loads(line.split(' ', 1)[1])
            if type(row['celsius']) not in (int, float):
                raise ValueError('celsius must be numeric')
            if row['celsius'] >= 85:
                return 'temperature-85C'
        elif line.startswith('GT710_BENCH '):
            row = json.loads(line.split(' ', 1)[1])
            if type(row['completion']) is not bool:
                raise ValueError('completion must be boolean')
            if not row['completion']:
                return 'completion-failed'
        elif line.startswith('GT710_MEMORY_READ '):
            row = json.loads(line.split(' ', 1)[1])
            if any(type(row[key]) is not int for key in ('readback_khz', 'post_khz')):
                raise ValueError('memory clocks must be integers')
            if row['readback_khz'] != row['post_khz']:
                return 'memory-clock-changed'
    except (ValueError, KeyError, TypeError) as error:
        report(f'WATCH_PARSE_ERROR {error}: {line}')
    return None


def capture(boot, head):
    directory = Path('/var/lib/breenix-metal/boots') / boot
    offset, pending, initialized = 0, '', False
    log(f'WATCH_READY {boot} expected_commit={head}')
    while True:
        serial = directory / 'serial.txt'
        try:
            if serial.exists():
                with serial.open(errors='replace') as stream:
                    stream.seek(offset)
                    pending += stream.read()
                    offset = stream.tell()
                while '\n' in pending:
                    line, pending = pending.split('\n', 1)
                    log(line)
                    if line.startswith('GT710_RAM_INIT_COMPLETE '):
                        initialized = True
                    # The step forbids a memory-clock change after RAM init.
                    if line.startswith('GT710_MEMORY_READ ') and not initialized:
                        continue
                    reason = stop_reason(line)
                    if reason:
                        log('WATCH_STOP ' + reason)
                        return STOP_REQUESTED
            if (directory / 'ended').exists():
                log('WATCH_ENDED without a stop request')
                return 0
        except (OSError, ValueError) as error:
            log(f'WATCH_READ_ERROR {error}; continuing')
        time.sleep(.02)


def run_on_host(boot, head):
    # Read the deployed file on the boot host and run those identical bytes in
    # the service container. METAL_INSTANCE comes only from the host site file.
    child = subprocess.Popen(
        ['sudo', '-n', 'incus', 'exec', os.environ['METAL_INSTANCE'], '--',
         'python3', '-u', '-c', Path(__file__).read_text(), '--capture', boot, head],
        stdout=subprocess.PIPE, text=True)
    for line in child.stdout:
        print(line, end='', flush=True)
    code = child.wait()
    if code != STOP_REQUESTED:
        log(f'WATCH_EXIT {code}; no metal-run stop requested')
        return code
    subprocess.run([sys.executable, str(Path.home() / '.local/share/breenix-metal/metal-run'),
                    'stop', boot], check=True)
    log('WATCH_STOP_ISSUED ' + boot)
    return 0


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--capture', action='store_true', help=argparse.SUPPRESS)
    parser.add_argument('boot')
    parser.add_argument('head')
    args = parser.parse_args()
    if not re.fullmatch(r'\d{8}T\d{6}Z-metal', args.boot):
        parser.error('invalid boot id')
    if not re.fullmatch(r'[0-9a-f]{40}', args.head):
        parser.error('invalid commit')
    return capture(args.boot, args.head) if args.capture else run_on_host(args.boot, args.head)


if __name__ == '__main__':
    sys.exit(main())
