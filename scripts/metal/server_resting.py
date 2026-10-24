"""Automated R240 resting-state category, with a resumable unlocked 24-hour phase."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import select
import subprocess
import tempfile
import time


class Lab:
    def __init__(self, api, args, scratch):
        self.api, self.args = api, args
        self.runner = api['runpy'].run_path(str(Path(api['__file__']).with_name('metal-run')))
        self.env = self.runner['host_env']()
        self.site = json.loads(self.remote('python3', '-c',
            'import sys,json;sys.path.insert(0,"/opt/breenix-metal");import metal_lib;'
            's=metal_lib.load_site();print(json.dumps({k:s.get(k) for k in '
            '("SERVER_SSH_HOST","OPERATOR_USER","OPERATOR_UID","OPERATOR_PASSWORD",'
            '"OPERATOR_SSH_PUBLIC_KEY","CONSOLE_USER","CONSOLE_PASSWORD")}))').stdout)
        identity = next((p.with_suffix('') for p in (Path.home() / '.ssh').glob('*.pub')
            if p.read_text().split()[:2] == self.site['OPERATOR_SSH_PUBLIC_KEY'].split()[:2]
            and p.with_suffix('').is_file()), None)
        if identity is None:
            raise ValueError('no operator private key on boot host')
        self.probe = api['Probe'](self.site['SERVER_SSH_HOST'], self.site['OPERATOR_USER'], identity, scratch)
        self.current = args.boot

    def remote(self, *cmd, **kw):
        return self.runner['in_metal'](self.env, *cmd, capture_output=True, text=True, **kw)

    def read(self, boot, name):
        result = self.remote('cat', '/var/lib/breenix-metal/boots/' + boot + '/' + name)
        return result.stdout if result.returncode == 0 else ''

    def save(self, name, value):
        self.remote('python3', '-c', 'import sys,json;sys.path.insert(0,"/opt/breenix-metal");'
            'import metal_lib as m;m.write_json(m.boot_dir(sys.argv[1])/sys.argv[2],json.load(sys.stdin))',
            self.args.boot, name, input=json.dumps(value), stdin=None, check=True)

    def lease(self):
        result = self.remote('cat', '/var/lib/breenix-metal/lease.json')
        return json.loads(result.stdout) if result.returncode == 0 else {}

    def locked(self):
        return self.remote('test', '-d', '/var/lib/breenix-metal/branch-tools.lock').returncode == 0

    def wait(self, predicate, seconds, why):
        until = time.monotonic() + seconds
        while True:
            value = predicate()
            if value:
                return value
            if time.monotonic() >= until:
                raise ValueError(why)
            time.sleep(2)

    def events(self, boot):
        return [json.loads(line) for line in self.read(boot, 'events.jsonl').splitlines()]

    def launch(self, *words):
        result = subprocess.run(['python3', str(Path(self.api['__file__']).with_name('metal-run')), *words],
            capture_output=True, text=True, timeout=300)
        if result.returncode:
            raise ValueError('boot launch refused: ' + result.stderr[-300:])
        ids = re.findall(r'"id":\s*"(\d{8}T\d{6}Z-metal)"', result.stdout)
        if not ids:
            raise ValueError('boot launch returned no identity')
        return ids[0]

    def ended(self, boot):
        return bool(self.read(boot, 'ended'))

    def pin(self):
        self.wait(lambda: 'bsshd: listening on ' in self.read(self.current, 'serial.txt'), 600, 'SSH never started')
        console = self.api['Console'](lambda *c, **kw: self.runner['in_metal'](self.env, *c, **kw),
            self.current, '/var/lib/breenix-metal/boots/' + self.current,
            self.site['CONSOLE_USER'], self.site['CONSOLE_PASSWORD'])
        try:
            self.probe.run_case('host-key', console.service_log('bsshd'))
        finally:
            console.logout()

    def boot_id(self):
        result = self.probe.ssh('cat /run/breenix/boot-id')
        self.probe.require(result)
        return result.stdout.strip()

    def idle(self, source):
        self.wait(lambda: self.ended(source), 1800, 'test did not finish')
        def candidate():
            lease = self.lease()
            return lease.get('id') if lease.get('id') != source and lease.get('kind') == 'persistent' else None
        self.current = self.wait(candidate, 120, 'empty queue did not start a persistent boot')
        self.wait(lambda: 'bsshd: listening on ' in self.read(self.current, 'serial.txt'), 600, 'idle SSH never started')
        self.probe.require(self.probe.ssh('id -u'), self.site['OPERATOR_UID'])
        run = json.loads(self.read(self.current, 'run.json'))
        serial = self.read(self.current, 'serial.txt')
        entry = re.findall(r'breenix-manager: booting (current|previous) [a-f0-9]{40}', serial)
        return dict(boot=self.current, source=source, sourceEndedAt=float(self.read(source, 'ended')),
            startedAt=run['startedAt'], run=run, serial=serial, entry=entry[-1] if entry else None,
            authenticated=True, strictHostKey=True, queueEmpty=not self.lease().get('queue'))

    def slots(self):
        result = self.remote('python3', '-c', 'import sys,json;sys.path.insert(0,"/opt/breenix-metal");'
            'import metal_persist;print(json.dumps(metal_persist.read_disk()["slots"]))', check=True)
        return json.loads(result.stdout)

    def power(self, action):
        """Authenticate sudo over SSH; the boot driver alone acts on the clean receipt."""
        process = subprocess.Popen(self.probe.common + ['-i', str(self.probe.identity), '-tt',
            self.probe.user + '@' + self.probe.host, 'sudo /sbin/' + action],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
        output = b''
        until = time.monotonic() + 25
        try:
            while b'[sudo] password for ' not in output:
                if time.monotonic() >= until:
                    raise ValueError('sudo did not prompt for authentication')
                if select.select([process.stdout], [], [], .2)[0]:
                    chunk = os.read(process.stdout.fileno(), 4096)
                    if not chunk:
                        raise ValueError('sudo ended before authentication')
                    output += chunk
            process.stdin.write(self.site['OPERATOR_PASSWORD'].encode() + b'\n')
            process.stdin.flush()
            # init terminates the SSH session during its stop sequence.
            process.wait(timeout=120)
        finally:
            if process.poll() is None:
                process.terminate()
                process.wait(timeout=10)


def run(api, args):
    checks = api['runpy'].run_path(str(Path(api['__file__']).with_name('resting_cases.py')))
    with tempfile.TemporaryDirectory(prefix='server-resting-') as scratch:
        lab = Lab(api, args, Path(scratch))
        if args.resting_phase == 'events':
            if not lab.locked():
                raise ValueError('machine-event acceptance requires the branch-tools lock')
            records = dict(boot=args.boot, commit=args.sha, milestone='server/resting-state', cases=[],
                evidence={}, cleanup=True, consoleGaps=[])
            lab.pin()
            print('SUITE server START cases=6', flush=True)
            for name, _ in checks['CASES'][:-1]:
                started = time.monotonic()
                try:
                    if name == 'clean-preempt':
                        source = lab.current
                        queued = time.time()
                        test = lab.launch('boot', '--branch', args.branch, '--sha', args.sha, '--no-wait')
                        records['testBoot'] = test
                        lab.wait(lambda: lab.ended(source), 120, 'queued test did not cleanly preempt persistent session')
                        lab.wait(lambda: any(e['event'] == 'lease-claimed' for e in lab.events(test)), 60, 'test never acquired released lease')
                        value = dict(run=json.loads(lab.read(source, 'run.json')), exit=lab.read(source, 'exit-status').strip(),
                            serial=lab.read(source, 'serial.txt'), events=lab.events(source), queuedAt=queued,
                            testClaimedAt=next(e['t'] for e in lab.events(test) if e['event'] == 'lease-claimed'))
                        checks['clean'](value, args.sha)
                    elif name == 'idle-return':
                        test = records['testBoot']
                        value = lab.idle(test)
                        if lab.read(test, 'exit-status').strip() != '0':
                            raise ValueError('preempting test boot failed')
                        build = json.loads(lab.read(test, 'delivery.json'))['build']
                        if build['commit'] != args.sha or json.loads(lab.read(test, 'run.json'))['toolsCommit'] != args.sha:
                            raise ValueError('test boot has wrong kernel or tools')
                        checks['returned'](value, args.sha)
                    elif name == 'one-shot':
                        before = lab.slots()
                        once = lab.launch('boot', '--persistent', '--entry', 'previous', '--verify', '--no-wait')
                        back = lab.idle(once)
                        serial = lab.read(once, 'serial.txt')
                        if lab.read(once, 'exit-status').strip() != '0':
                            raise ValueError('one-shot previous boot did not complete')
                        entry = re.findall(r'breenix-manager: booting (current|previous) [a-f0-9]{40}', serial)
                        value = dict(onceRun=json.loads(lab.read(once, 'run.json')), onceEntry=entry[-1] if entry else None,
                            beforeSlots=before, afterSlots=lab.slots(), returned=back)
                        checks['one_shot'](value, args.sha)
                    elif name == 'reboot-return':
                        offset = len(lab.events(lab.current))
                        before = json.loads(lab.read(lab.current, 'run.json'))
                        old_id = lab.boot_id()
                        lab.power('reboot')
                        lab.wait(lambda: any(e['event'] == 'requested-reboot-ready' for e in lab.events(lab.current)[offset:]),
                            600, 'reboot did not return unaided')
                        serial = lab.read(lab.current, 'serial.txt')
                        entry = re.findall(r'breenix-manager: booting (current|previous) [a-f0-9]{40}', serial)
                        value = dict(beforeRun=before, afterRun=json.loads(lab.read(lab.current, 'run.json')),
                            beforeId=old_id, afterId=lab.boot_id(), entry=entry[-1] if entry else None,
                            events=lab.events(lab.current)[offset:], authenticated=True, strictHostKey=True)
                        checks['reboot'](value, args.sha)
                    else:
                        target = api['Watchdog'](lab.probe, None, lab.remote, lab.runner, lab.env,
                            lab.current, args.sha, args.branch)
                        # Enqueue the fault without a runner stop: it must cleanly preempt too.
                        target.stopped = True
                        target.run_case('feeder-hang')
                        value = json.loads(lab.read(target.boots[-1], 'watchdog-verdict.json'))
                        checks['watchdog'](value, args.sha)
                        lab.idle(target.boots[-1])
                    records['evidence'][name] = value
                    result, reason = 'PASS', ''
                except Exception as error:
                    result, reason = 'FAIL', str(error)
                records['cases'].append(dict(id='resting-state/' + name, result=result, reason=reason,
                    ms=int((time.monotonic() - started) * 1000)))
                print('SUITE server CASE resting-state/' + name + ' ' + result + ' msg=' + reason, flush=True)
                records['session'] = lab.current
                records['known'] = lab.probe.known.read_text()
                lab.save('resting-prepared.json', records)
                if result == 'FAIL':
                    # Cancel only this runner's queued boot; preserve the live baseline.
                    test = records.get('testBoot')
                    if test and not lab.ended(test):
                        subprocess.run(['python3', str(Path(api['__file__']).with_name('metal-run')), 'stop', test], check=True)
                    return 1
            print('Machine events complete; restore main tools and release the lock before --resting-phase soak', flush=True)
            return 0

        records = json.loads(lab.read(args.boot, 'resting-prepared.json'))
        if records['commit'] != args.sha or len(records['cases']) != 5 or any(c['result'] != 'PASS' for c in records['cases']):
            raise ValueError('machine events have not passed at this head')
        lab.current = records['session']
        lab.probe.known.write_text(records['known'])
        if args.resting_phase == 'soak':
            if lab.locked():
                raise ValueError('release the branch-tools lock before the 24-hour session')
            value = dict(boot=lab.current, run=json.loads(lab.read(lab.current, 'run.json')), samples=[])
            checks['persistent'](value['run'], args.sha)
            start = time.monotonic()
            # Deadlines are measured on this boot host, never by an adjustable guest clock.
            for i in range(289):
                deadline = start + i * 300
                while time.monotonic() < deadline:
                    time.sleep(min(30, deadline - time.monotonic()))
                at = time.time()
                lock = lab.locked()
                lease = lab.lease().get('id')
                boot_id = lab.boot_id()
                value['samples'].append(dict(at=at, bootId=boot_id, lease=lease, lockHeld=lock,
                    authenticated=True, strictHostKey=True))
                value['elapsedMonotonic'] = time.monotonic() - start
                lab.save('resting-soak.json', value)
                if lock or lease != lab.current or boot_id != value['samples'][0]['bootId']:
                    raise ValueError('24-hour persistent session was interrupted or tools lock was held')
                print('Persistent SSH probe ' + str(i) + '/288 passed', flush=True)
            checks['uptime'](value, args.sha)
            return 0

        if not lab.locked():
            raise ValueError('final resting-state scoring requires the branch-tools lock')
        value = json.loads(lab.read(args.boot, 'resting-soak.json'))
        checks['uptime'](value, args.sha)
        records['evidence']['uptime-24h'] = value
        records['cases'].append(dict(id='resting-state/uptime-24h', result='PASS', reason='', ms=86400000))
        checks['validate'](records['evidence'], args.sha)
        # The original baseline retains the definition and authenticated pinning capture.
        boot = json.loads(lab.read(args.boot, 'run.json'))
        serial = lab.read(args.boot, 'serial.txt')
        verdict = api['score'](json.loads(lab.read(args.boot, 'definition/docs/suites/server.json')),
            json.loads(lab.read(args.boot, 'definition/docs/efforts/path.json')), records, serial, boot, args.sha, ['KERNEL PANIC:'])
        lab.save('server-resting-state.json', records)
        lab.remote('python3', '-c', api['WATCHDOG_ARCHIVE'], '/var/lib/breenix-metal/boots/' + args.boot, check=True)
        for case in records['cases']:
            print('SUITE server CASE ' + case['id'] + ' PASS ms=' + str(case['ms']), flush=True)
        print('SUITE server DONE passed=6 failed=0 skipped=0 total=6\n' + verdict, flush=True)
        return 0
