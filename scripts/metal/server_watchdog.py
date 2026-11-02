"""ARM64 QEMU watchdog cases and judge; metal keeps its existing S19 path."""
import json
import math
import os
from pathlib import Path
import re
import runpy
import subprocess
import tempfile
import time
import uuid


CASES = ('feeder-hang', 'health-hang')
BOOT_ID = r'[0-9a-f]+-[0-9a-f]+-[0-9a-f]+'


def score_case(value, baseline, sha, fatal_patterns):
    """Judge retained machine and SSH evidence, never a precomputed PASS flag."""
    before, after = value.get('beforeRun', {}), value.get('afterRun', {})
    receipt = value.get('receipt', {})
    name = receipt.get('case')
    if name not in CASES or value.get('boot') != before.get('id') or before.get('id') == after.get('id'):
        raise ValueError('watchdog needs a distinct fault boot and successor for this case')
    if not baseline.get('runId') or not before.get('qemuProcess'):
        raise ValueError('watchdog has no run or QEMU process identity')
    for record in (before, after):
        if (record.get('target'), record.get('mode'), record.get('bootedCommit'), record.get('toolsCommit'),
                record.get('evidenceEligible'), record.get('kind'), record.get('runId'), record.get('qemuProcess')) != (
                'qemu', 'persistent', sha, sha, True, 'fault-test', baseline['runId'], before['qemuProcess']):
            raise ValueError('watchdog successor changed run, process, kernel or tools')
    build = value.get('build', {})
    if (build.get('commit'), build.get('faultKind'), build.get('evidenceEligible')) != (sha, 'watchdog', True):
        raise ValueError('watchdog has no verified watchdog fault kernel at this commit')
    events = value.get('events', [])
    def one(kind):
        found = [e for e in events if e.get('event') == kind]
        if len(found) != 1: raise ValueError('watchdog needs exactly one ' + kind)
        return found[0]
    trigger, expired, reset, successor = [one(k) for k in
        ('watchdog-trigger', 'WATCHDOG', 'RESET', 'watchdog-reset-observed')]
    if (not events.index(trigger) < events.index(expired) < events.index(reset) < events.index(successor) or
            expired.get('data', {}).get('action') != 'reset' or
            reset.get('data', {}).get('reason') != 'guest-reset' or
            reset.get('data', {}).get('guest') is not True or successor.get('successor') != after.get('id')):
        raise ValueError('watchdog has no ordered autonomous reset successor')
    # QMP defines WATCHDOG/reset -> RESET. Any other stop/reset is intervention.
    forbidden = {'STOP', 'SHUTDOWN', 'host-stop', 'host-reset', 'console-gap', 'requested-shutdown'}
    if any(e.get('event') in forbidden for e in events + value.get('afterEvents', [])) or any(
            e.get('event') in {'RESET', 'WATCHDOG', 'watchdog-reset-observed'} for e in value.get('afterEvents', [])):
        raise ValueError('host intervention or another transition before watchdog SSH proof')
    start, observed, ssh_at = trigger.get('t'), successor.get('t'), receipt.get('sshAt')
    if (not all(isinstance(t, (int, float)) and math.isfinite(t) for t in (start, observed, ssh_at)) or
            not start <= observed <= ssh_at <= start + 600 or receipt.get('trigger') != start):
        raise ValueError('watchdog SSH proof is outside its trigger deadline')
    # Fault injection follows the preceding 20-second feed interval, leaving
    # at least 100 seconds on the programmed 120-second timer.
    stamp = expired.get('timestamp', {})
    if (not isinstance(stamp, dict) or
            any(type(stamp.get(key)) is not int for key in ('seconds', 'microseconds')) or
            not 0 <= stamp['microseconds'] < 1_000_000):
        raise ValueError('watchdog expiry has no QMP timestamp')
    expired_at = stamp['seconds'] + stamp['microseconds'] / 1_000_000
    if not start + 100 <= expired_at <= observed:
        raise ValueError('watchdog expiry is outside its 120-second timer window')
    serial, returned = value.get('beforeSerial', ''), value.get('afterSerial', '')
    lines = serial.splitlines()
    triggers = [i for i, line in enumerate(lines) if line.startswith('[watchdog-fault] trigger')]
    if len(triggers) != 1 or lines[triggers[0]] != '[watchdog-fault] trigger ' + name:
        raise ValueError('serial does not contain exactly this case trigger')
    pre = '\n'.join(lines[:triggers[0]])
    arm = re.search(r'^\[watchdog\] armed seconds=120 action=hard-reset(?: .*|)$', pre, re.M)
    healthy = [(m.start(), int(m[1])) for m in re.finditer(r'^\[watchdog\] healthy feeds=(\d+)$', pre, re.M)]
    if not arm or not any(at > arm.start() and rounds >= 6 for at, rounds in healthy):
        raise ValueError('watchdog was not armed for 120 seconds and healthy for six rounds before trigger')
    # U2: QEMU 11.0.3 retains the ESB timeout flag across reset in one process.
    if not re.search(r'^\[watchdog\] previous-expired=1\r?$', returned, re.M):
        raise ValueError('successor has no retained watchdog expiry flag')
    old_ids = re.findall(r'^\[init\] log boot-id=(' + BOOT_ID + r')\r?$', pre, re.M)
    new_ids = re.findall(r'^\[init\] log boot-id=(' + BOOT_ID + r')\r?$', returned, re.M)
    if (len(old_ids) != 1 or len(new_ids) != 1 or old_ids[0] == new_ids[0] or
            receipt.get('bootId') != new_ids[0] or receipt.get('boot') != after.get('id') or
            receipt.get('commit') != sha or receipt.get('exit') != 0 or receipt.get('strictHostKey') is not True or
            'bsshd: listening on 0.0.0.0:22' not in returned):
        raise ValueError('successor has no fresh strict-key SSH boot-id read')
    if any(re.search(pattern, text) for pattern in fatal_patterns + [r'KERNEL PANIC:'] for text in (serial, returned)):
        raise ValueError('fatal output during watchdog recovery')


def score_qemu(records, boot, sha, fatal_patterns):
    values = records.get('faultBoots', [])
    if ([v.get('receipt', {}).get('case') for v in values] != list(CASES) or
            len({v.get('boot') for v in values}) != len(CASES) or
            len({v.get('beforeRun', {}).get('qemuProcess') for v in values}) != len(CASES)):
        raise ValueError('watchdog needs both distinct fault launches in case order')
    for value in values:
        score_case(value, boot, sha, fatal_patterns)


class QemuWatchdog:
    def __init__(self, machine, probe, site, sha, fatal):
        self.machine, self.probe, self.site, self.sha, self.fatal = machine, probe, site, sha, fatal
        self.baseline = machine.identity()
        self.values = []

    def poweroff(self, machine):
        if not machine.holds_lease(): raise ValueError('watchdog boot lost its machine lease')
        machine.stop_for_fault(self.site['CONSOLE_USER'], self.site['CONSOLE_PASSWORD'])
        until = time.monotonic() + 120
        while not machine.ended(machine.boot) or (machine.run / 'boot.lock').exists():
            if time.monotonic() >= until: raise ValueError('watchdog boot did not power off cleanly')
            time.sleep(.2)
        events = json.loads('[' + ','.join(machine.read('events.jsonl').splitlines()) + ']')
        requests = [i for i, e in enumerate(events) if e.get('event') == 'requested-shutdown' and e.get('action') == 'poweroff']
        if not requests or not any(e.get('event') == 'SHUTDOWN' and e.get('data', {}).get('reason') == 'guest-shutdown'
                                   for e in events[requests[-1] + 1:]):
            raise ValueError('watchdog cleanup has no authenticated guest poweroff')
        # This helper seals only an authenticated guest poweroff, after QEMU exits.
        # Fault launches preserve the disks too; their next case needs this seal.
        runpy.run_path(str(Path(__file__).resolve().parent.parent / 'vm-artifacts.py'))['continuation'](machine.run)

    def run_case(self, name):
        boot = self.machine.start_watchdog_fault(self.sha, name)
        fault = self.machine.select(boot)
        value = dict(boot=boot, commit=self.sha, passed=False)
        current = fault
        try:
            value['build'] = fault.fault_build()
            deadline = time.monotonic() + 1200  # startup; replaced at first trigger
            trigger = None
            while time.monotonic() < deadline:
                events = [json.loads(line) for line in fault.read('events.jsonl').splitlines()]
                triggers = [e for e in events if e.get('event') == 'watchdog-trigger']
                if len(triggers) > 1: raise ValueError('duplicate watchdog trigger')
                if triggers and trigger is None:
                    trigger = triggers[0]['t']
                    deadline = time.monotonic() + max(0, 600 - (time.time() - trigger))
                resets = [e for e in events if e.get('event') == 'watchdog-reset-observed']
                if len(resets) > 1: raise ValueError('duplicate watchdog successor')
                if resets:
                    current = fault.select(resets[0]['successor'])
                    if not current.holds_lease(): raise ValueError('watchdog successor lost its lease')
                    serial = current.read('serial.txt')
                    if 'bsshd: listening on 0.0.0.0:22' in serial:
                        site = current.reboot_site()
                        # Readdress the original console-pinned key, without a new
                        # keyscan: each launch can have a different forwarded port.
                        if 'StrictHostKeyChecking=yes' not in self.probe.common or not self.probe.known.is_file():
                            raise ValueError('watchdog probe has no strict pinned host key')
                        pinned = self.probe.known.read_text().split()
                        if len(pinned) != 3 or pinned[1] != 'ssh-ed25519':
                            raise ValueError('watchdog probe has no unique Ed25519 pin')
                        self.probe.port = int(site['SERVER_SSH_PORT'])
                        self.probe.common[self.probe.common.index('-p') + 1] = str(self.probe.port)
                        self.probe.known.write_text('[' + self.probe.host + ']:' + str(self.probe.port) +
                                                    ' ' + ' '.join(pinned[1:]) + '\n')
                        result = self.probe.ssh('cat /run/breenix/boot-id', timeout=max(1, min(45, deadline - time.monotonic())))
                        ssh_at = time.time()
                        if result.returncode == 0 and re.fullmatch(BOOT_ID + r'\s*', result.stdout):
                            value.update(beforeRun=fault.identity(), afterRun=current.identity(),
                                beforeSerial=fault.read('serial.txt'), afterSerial=current.read('serial.txt'),
                                events=events, afterEvents=[json.loads(line) for line in current.read('events.jsonl').splitlines()],
                                receipt=dict(boot=current.boot, commit=self.sha, case=name, trigger=trigger,
                                             sshAt=ssh_at, bootId=result.stdout.strip(), strictHostKey=True, exit=result.returncode))
                            score_case(value, self.baseline, self.sha, self.fatal)
                            value['passed'] = True
                            break
                elif fault.ended(boot):
                    raise ValueError('fault boot ended without a watchdog successor')
                time.sleep(1)
            if not value['passed']: raise ValueError('no unaided strict-key SSH recovery within trigger deadline')
        except Exception as error:
            value['error'] = type(error).__name__ + ': ' + str(error)
            raise
        finally:
            # Verdict precedes cleanup, so the deliberate guest poweroff cannot
            # masquerade as recovery, and failed cases remain available too.
            fault.write('watchdog-verdict.json', value)
            self.values.append(value)
            self.machine = current
        self.poweroff(current)


def run(api, args):
    machine = api['QemuTarget'](args.run_dir, args.boot)
    baseline = machine.identity()
    if ((baseline.get('bootedCommit'), baseline.get('toolsCommit'), baseline.get('mode'), baseline.get('evidenceEligible')) !=
            (args.sha, args.sha, 'persistent', True) or not machine.holds_lease()):
        raise ValueError('watchdog needs this exact persistent kernel and tools with the machine lease')
    manifest = json.loads(machine.read('definition/docs/suites/server.json'))
    api['validate_manifest'](manifest)
    effort = json.loads(machine.read('definition/docs/efforts/path.json'))
    fatal = json.loads(machine.read('definition/docs/boot-path.json'))['fatal']
    site = machine.reboot_site()
    identity = next((p.with_suffix('') for p in (Path.home() / '.ssh').glob('*.pub')
                     if p.read_text().split()[:2] == site['OPERATOR_SSH_PUBLIC_KEY'].split()[:2]
                     and p.with_suffix('').is_file()), None)
    if identity is None: raise ValueError('watchdog has no private key matching the site public key')
    records = dict(boot=args.boot, commit=args.sha, milestone='server/watchdog', cases=[], cleanup=False, consoleGaps=[])
    suite_lines = []
    def emit(line):
        suite_lines.append(line)
        print(line, flush=True)
    with tempfile.TemporaryDirectory(prefix='server-watchdog-') as scratch:
        probe = api['Probe'](site['SERVER_SSH_HOST'], site['OPERATOR_USER'], identity,
                             Path(scratch), port=int(site['SERVER_SSH_PORT']))
        console = machine.console(site['CONSOLE_USER'], site['CONSOLE_PASSWORD'])
        target = QemuWatchdog(machine, probe, site, args.sha, fatal)
        try:
            probe.run_case('host-key', console.service_log('bsshd'))
            console.logout()
            target.poweroff(machine)
            emit('SUITE server START cases=2')
            for name in CASES:
                started = time.monotonic()
                try:
                    target.run_case(name)
                    result, reason = 'PASS', ''
                except Exception as error:
                    result, reason = 'FAIL', type(error).__name__ + ': ' + str(error)
                case = dict(id='watchdog/' + name, result=result, reason=reason, ms=int((time.monotonic() - started) * 1000))
                records['cases'].append(case)
                emit(f"SUITE server CASE {case['id']} {result} ms={case['ms']}" + (' msg=' + reason if reason else ''))
                if result == 'FAIL':
                    # Preserve the fault VM on failure for diagnosis; do not recover it.
                    break
            records['cleanup'] = len(records['cases']) == 2 and all(c['result'] == 'PASS' for c in records['cases'])
        finally:
            probe.cleanup()
            records['faultBoots'] = target.values
            machine.write('server-watchdog-transcript.json', probe.transcript)
            machine.write('server-watchdog.json', records)
    serial = machine.read('serial.txt')
    records['consoleGaps'] = [e for e in map(json.loads, machine.read('events.jsonl').splitlines()) if e.get('event') == 'console-gap']
    try:
        verdict = api['score'](manifest, effort, records, serial, baseline, args.sha, fatal)
        status = 0
    except ValueError as error:
        verdict, status = 'FAIL: ' + str(error), 1
    records['verdict'] = verdict
    machine.write('server-watchdog.json', records)
    passed = sum(c['result'] == 'PASS' for c in records['cases'])
    emit(f'SUITE server DONE passed={passed} failed={2-passed} skipped=0 total=2')
    suite_file = machine.directory / 'watchdog-suite.txt'
    suite_file.write_text('\n'.join(suite_lines) + '\n')
    subprocess.run([str(Path(api['__file__']).resolve().parent.parent / 'vigil-record.sh'),
                    'record', 'qemu', 'persistent', 'server', str(suite_file), str(machine.directory / 'serial.txt'),
                    (args.run_dir / 'qemu-profile').read_text().strip(),
                    time.strftime('%Y%m%dT%H%M%SZ', time.gmtime()) + '-qemu-' + uuid.uuid4().hex,
                    args.sha, (machine.directory / 'started').read_text().strip(), str(time.time()), str(status)],
                   check=True, env=dict(os.environ, BREENIX_VIGIL_ARCH='aarch64'))
    print(verdict, flush=True)
    return status
