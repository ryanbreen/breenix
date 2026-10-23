"""Boot-host side of S21. Private SSH keys stay on the boot host.

The leased driver alone powers the machine and chooses rollback; this worker only
answers each verification boot with an authenticated receipt.
"""
import json
from pathlib import Path
import subprocess
import sys
import time


def watch(runner, install):
    env = runner['host_env']()
    root = runner['STATE'] + '/boots/'
    seen = set()

    def read(boot, name):
        result = runner['in_metal'](env, 'cat', root + boot + '/' + name, capture_output=True, text=True)
        return result.stdout if result.returncode == 0 else ''

    # Building and waiting for the lease precede power-on and have their own bounds.
    while True:
        events = [json.loads(line) for line in read(install, 'events.jsonl').splitlines()]
        for event in events:
            if event.get('event') not in ('persistent-verification-next', 'persistent-rollback-next'):
                continue
            boot = event['verification']
            if boot in seen:
                continue
            while not read(boot, 'ended'):
                power = [json.loads(line) for line in read(boot, 'events.jsonl').splitlines()
                         if json.loads(line).get('event') == 'power-on']
                if not power:
                    time.sleep(1)
                    continue
                t0 = power[-1]['t']
                # observed_at is the queue's power-on dispatch time, recorded as t.
                left = t0 + 600 - time.time()
                if left <= 0:
                    break
                try:
                    subprocess.run([sys.executable, str(Path(__file__).with_name('server-suite')),
                                    '--probe-upgrade', '--boot', boot], timeout=left, check=False)
                except subprocess.TimeoutExpired:
                    pass
                if read(boot, 'ssh-verification.json'):
                    break
                time.sleep(1)
            seen.add(boot)
        if read(install, 'upgrade-result.json'):
            return 0
        if read(install, 'ended') and not events:
            return 1
        if read(install, 'exit-status').strip() not in ('', '0'):
            return 1
        time.sleep(1)
