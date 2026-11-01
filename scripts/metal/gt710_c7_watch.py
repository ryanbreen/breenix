#!/usr/bin/env python3
"""Watch one lane-owned C7 production boot from the boot host, using its deployed tools.

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


def stop_reason(line, startup_only=False):
    if line.startswith(('GT710_CLOCK_FALLBACK ', 'GT710_CLOCK_GUARD ', 'GT710_C5F_ERROR ',
                        'GT710_NATIVE_RESET_REFUSAL ', 'GT710_GR_REFUSAL_CAPTURE_FAILED ',
                        'PANIC', 'LOCKUP', 'panicked at')):
        return 'clock-or-device-fault'
    if line.startswith('GT710_NATIVE_FAULT ') and 'reason=shared core requested reset ' not in line:
        return 'native-device-fault'
    if line.startswith('GT710_CLOCK_EXEC ') and 'status=-1 ' in line:
        return 'EXEC-failed'
    if line.startswith('GT710_C5F_HEALTH ') and '"healthy":false' in line:
        # The post-EXEC boundary may clear one isolated sticky underflow.
        # Its explicit ERROR, or the following failed startup, decides refusal.
        return None
    if line.startswith('GT710_CLOCK_STARTUP '):
        row = dict(re.findall(r'(\w+)=([^ ]+)', line))
        if row.get('full') != 'true' or row.get('status') != 'ok':
            return 'startup-not-full-clocks'
        for key, target in [('gpc_khz',1307000),('gpc_counter_khz',81688),('memory_khz',900000)]:
            if abs(int(row[key])-target)>target//100:
                return key+'-outside-one-percent'
        if int(row['temperature_c'])>=85:
            return 'temperature-85C'
        if startup_only:
            return 'startup-observed'
    if line.startswith('GT710_CLOCK_TEMPERATURE '):
        row = dict(re.findall(r'(\w+)=([^ ]+)', line))
        if int(row['celsius'])>=85 or row['tripped']=='true':
            return 'thermal-guard-trip'
    return None


def capture(boot, head, startup_only=False):
    directory = Path('/var/lib/breenix-metal/boots') / boot
    offset, pending = 0, ''
    record=json.loads((directory / 'run.json').read_text())
    if record.get('id')!=boot or record.get('commit')!=head or record.get('toolsCommit')!=head:
        raise RuntimeError('boot/build/tools identity mismatch; refusing to observe or stop')
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
                    try:
                        reason = stop_reason(line,startup_only=startup_only)
                    except (ValueError, KeyError) as error:
                        log(f'WATCH_PARSE_ERROR {error}; continuing')
                        reason = None
                    if reason:
                        log('WATCH_STOP ' + reason)
                        return STOP_REQUESTED
            if (directory / 'ended').exists():
                log('WATCH_ENDED without a stop request')
                return 0
        except (OSError, ValueError) as error:
            log(f'WATCH_READ_ERROR {error}; continuing')
        time.sleep(.02)


def run_on_host(boot, head, startup_only=False):
    # Read the deployed file on the boot host and run those identical bytes in
    # the service container. METAL_INSTANCE comes only from the host site file.
    child = subprocess.Popen(
        ['sudo', '-n', 'incus', 'exec', os.environ['METAL_INSTANCE'], '--',
         'python3', '-u', '-c', Path(__file__).read_text(), '--capture', *(['--startup-only'] if startup_only else []), boot, head],
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
    parser.add_argument('--startup-only', action='store_true', help='stop after the clock startup record')
    parser.add_argument('boot')
    parser.add_argument('head')
    args = parser.parse_args()
    if not re.fullmatch(r'\d{8}T\d{6}Z-metal', args.boot):
        parser.error('invalid boot id')
    if not re.fullmatch(r'[0-9a-f]{40}', args.head):
        parser.error('invalid commit')
    return capture(args.boot, args.head, args.startup_only) if args.capture else run_on_host(args.boot, args.head, args.startup_only)


if __name__ == '__main__':
    sys.exit(main())
