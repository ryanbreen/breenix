#!/usr/bin/env python3
"""Orderly reboot of a persistent QEMU guest within the same process."""
import os
from pathlib import Path
import runpy
import sys
import time

ROOT = Path(__file__).resolve().parent


def main(run):
    suite = runpy.run_path(str(ROOT / 'metal/server-suite'))
    site = runpy.run_path(str(ROOT / 'vm_site.py'))['load_site']()
    target = suite['QemuTarget'](run)
    if not target.holds_lease():
        raise ValueError('this boot does not hold the machine lease')
    console = target.console(site['CONSOLE_USER'], site['CONSOLE_PASSWORD'])
    console.login()
    console.type_line('/sbin/reboot')
    timeout = float(os.environ.get('BREENIX_VM_REBOOT_TIMEOUT', '600'))
    if timeout <= 0:
        raise ValueError('reboot timeout must be positive')
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            new = (target.run / 'current').read_text().strip()
            if new and new != target.boot and target.ended(target.boot):
                new_target = suite['QemuTarget'](run, new)
                serial = new_target.read('serial.txt', required=False)
                if serial is not None and suite['Console'].LOGIN in serial and new_target.holds_lease():
                    print('old boot: ' + target.boot)
                    print('new boot: ' + new)
                    return
        except OSError:
            pass
        time.sleep(.1)
    raise ValueError('timed out waiting for the new boot and login prompt')


if __name__ == '__main__':
    try:
        main(sys.argv[1])
    except (OSError, ValueError) as error:
        raise SystemExit(str(error)) from error
