"""Automated R240 resting-state category under S20 and operator ruling S25."""
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
        def read():
            result = self.probe.ssh('cat /run/breenix/boot-id')
            # bsshd binds before DHCP completes. Only initial transport
            # readiness is retried; authentication and command failures fail.
            if (result.returncode == 255 and any(message in result.stderr
                    for message in ('Connection refused', 'Connection timed out', 'No route to host'))):
                return None
            self.probe.require(result)
            return result.stdout.strip()
        return self.wait(read, 600, 'persistent SSH transport never became ready')

    def idle(self, source):
        self.wait(lambda: self.ended(source), 1800, 'test did not finish')
        def candidate():
            lease = self.lease()
            return lease.get('id') if lease.get('id') != source and lease.get('kind') == 'persistent' else None
        self.current = self.wait(candidate, 120, 'empty queue did not start a persistent boot')
        self.wait(lambda: 'bsshd: listening on ' in self.read(self.current, 'serial.txt'), 600, 'idle SSH never started')
        self.boot_id()
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
                    preemption = lab.wait(lambda: lab.read(source, 'preemption.json'), 30, 'queued test has no preemption receipt')
                    value = dict(run=json.loads(lab.read(source, 'run.json')), exit=lab.read(source, 'exit-status').strip(),
                        serial=lab.read(source, 'serial.txt'), events=lab.events(source), queuedAt=queued,
                        test=test, preemption=json.loads(preemption),
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
                    once = lab.launch('boot', '--persistent', '--entry', 'previous', '--no-wait')
                    lab.current = once
                    lab.wait(lambda: 'bsshd: listening on ' in lab.read(once, 'serial.txt'), 600, 'previous slot did not reach SSH')
                    once_run = json.loads(lab.read(once, 'run.json'))
                    old_id = lab.boot_id()
                    lab.power('reboot')
                    lab.wait(lambda: any(e['event'] == 'requested-reboot-ready' for e in lab.events(once)),
                        600, 'one-shot reboot did not return to SSH')
                    new_id = lab.boot_id()
                    reset_run = json.loads(lab.read(once, 'run.json'))
                    entries = [e['entry'] for e in lab.events(once) if e['event'] == 'slot-boot']
                    serial = lab.read(once, 'serial.txt')
                    entry = re.findall(r'breenix-manager: booting (current|previous) [a-f0-9]{40}', serial)
                    lab.power('poweroff')
                    back = lab.idle(once)
                    if lab.read(once, 'exit-status').strip() != '0':
                        raise ValueError('one-shot session did not complete normally')
                    value = dict(onceRun=once_run, onceEntry=entry[0] if entry else None,
                        resetRun=reset_run, resetEntries=entries, beforeId=old_id, afterId=new_id,
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
            lab.save('server-resting-state.json', records)
            if result == 'FAIL':
                # Cancel only this runner's queued boot; preserve the live baseline.
                test = records.get('testBoot')
                if test and not lab.ended(test):
                    subprocess.run(['python3', str(Path(api['__file__']).with_name('metal-run')), 'stop', test], check=True)
                return 1
        started = time.monotonic()
        try:
            run = json.loads(lab.read(lab.current, 'run.json'))
            entries = [e['entry'] for e in lab.events(lab.current) if e['event'] == 'slot-boot']
            value = dict(boot=lab.current, run=run, bootId=lab.boot_id(), lease=lab.lease().get('id'),
                entry=entries[-1] if entries else None, authenticated=True, strictHostKey=True)
            checks['ssh'](value, args.sha)
            records['evidence']['returned-ssh'] = value
            result, reason = 'PASS', ''
        except Exception as error:
            result, reason = 'FAIL', str(error)
        records['cases'].append(dict(id='resting-state/returned-ssh', result=result, reason=reason,
            ms=int((time.monotonic() - started) * 1000)))
        lab.save('server-resting-state.json', records)
        print('SUITE server CASE resting-state/returned-ssh ' + result + ' msg=' + reason, flush=True)
        if result == 'FAIL':
            return 1
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
