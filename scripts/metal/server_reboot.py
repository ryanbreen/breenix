"""Machine-event cases for server/reboot; called by server-suite on the boot host."""
import hashlib
import json
import os
from pathlib import Path
import re
import select
import shlex
import subprocess
import tempfile
import time
import uuid


def verify_root_readback(root, token, output):
    """Compare complete checksum records; kernel output may separate records."""
    for name, expected in [('order', b'reboot-child\nreboot-base\n'), ('data', token.encode())]:
        digest = hashlib.sha256(expected).hexdigest()
        record = digest + '  ' + root + '/' + name
        if not re.search(r'^' + re.escape(record) + r'\r?$', output, re.M):
            raise ValueError('dependent did not stop first or root data was lost')



def run(api, args):
    cases = api['REBOOT']
    print('SUITE server START cases=' + str(len(cases)), flush=True)
    runner = api['runpy'].run_path(str(Path(api['__file__']).with_name('metal-run')))
    env = runner['host_env']()
    remote = lambda *cmd, **kw: runner['in_metal'](env, *cmd, capture_output=True, text=True, **kw)
    def read(boot, name):
        result = remote('cat', '/var/lib/breenix-metal/boots/' + boot + '/' + name)
        if result.returncode: raise ValueError('missing boot artifact ' + name)
        return result.stdout
    def save(boot, name, value):
        result = remote('tee', '/var/lib/breenix-metal/boots/' + boot + '/' + name,
                        stdin=None, input=json.dumps(value) + '\n')
        if result.returncode: raise ValueError('cannot save reboot evidence')
    def ended(boot):
        return remote('test', '-e', '/var/lib/breenix-metal/boots/' + boot + '/ended').returncode == 0
    def await_end(boot, seconds=120):
        deadline = time.monotonic() + seconds
        while not ended(boot):
            if time.monotonic() >= deadline: raise ValueError('requested shutdown did not end the boot')
            time.sleep(1)
    def boot_command(*words):
        result = subprocess.run(['python3', str(Path(api['__file__']).with_name('metal-run')), *words],
                                capture_output=True, text=True, timeout=15000)
        if result.returncode: raise ValueError('metal-run refused the next case boot: ' + result.stdout + result.stderr)
        found = re.search(r'\d{8}T\d{6}Z-metal', result.stdout)
        if not found: raise ValueError('metal-run returned no boot ID')
        return found[0]
    site_result = remote('python3', '-c', 'import sys,json;sys.path.insert(0,"/opt/breenix-metal");'
                         'import metal_lib; s=metal_lib.load_site(); print(json.dumps({k:s.get(k) for k in '
                         '("SERVER_SSH_HOST","OPERATOR_USER","OPERATOR_PASSWORD","OPERATOR_SSH_PUBLIC_KEY",'
                         '"CONSOLE_USER","CONSOLE_PASSWORD")}))')
    if site_result.returncode: raise ValueError('cannot read private site fields')
    site = json.loads(site_result.stdout)
    if not all(site.values()): raise ValueError('reboot cases need the operator and console site fields')
    identity = next((p.with_suffix('') for p in (Path.home() / '.ssh').glob('*.pub')
                     if p.read_text().split()[:2] == site['OPERATOR_SSH_PUBLIC_KEY'].split()[:2]
                     and p.with_suffix('').is_file()), None)
    if identity is None: raise ValueError('no private key matches the site key')
    current = args.boot
    records = dict(boot=current, commit=args.sha, milestone='server/reboot', cases=[], cleanup=True,
                   consoleGaps=[], transitions=[])
    with tempfile.TemporaryDirectory(prefix='server-reboot-') as scratch:
        probe = api['Probe'](site['SERVER_SSH_HOST'], site['OPERATOR_USER'], identity, Path(scratch))
        def connect():
            deadline = time.monotonic() + 600
            while True:
                boot = json.loads(read(current, 'run.json'))
                if ended(current): raise ValueError('boot ended before the reboot case could connect')
                capture = remote('cat', '/var/lib/breenix-metal/boots/' + current + '/serial.txt')
                if capture.returncode == 0 and 'bsshd: listening on 0.0.0.0:22' in capture.stdout: break
                if time.monotonic() >= deadline: raise ValueError('persistent boot never reached SSH')
                time.sleep(2)
            if (boot.get('bootedCommit'), boot.get('toolsCommit'), boot.get('mode'), boot.get('evidenceEligible')) != (args.sha, args.sha, 'persistent', True):
                raise ValueError('reboot evidence needs the exact kernel and tools commits')
            lease = remote('cat', '/var/lib/breenix-metal/lease.json')
            if lease.returncode or json.loads(lease.stdout).get('id') != current:
                raise ValueError('case boot has no machine lease')
            console = api['Console'](lambda *cmd, **kw: runner['in_metal'](env, *cmd, **kw), current,
                                    '/var/lib/breenix-metal/boots/' + current,
                                    site['CONSOLE_USER'], site['CONSOLE_PASSWORD'])
            probe.run_case('host-key', console.service_log('bsshd'))
            return console
        console = connect()
        resources = []
        for name, _ in cases:
            started = time.monotonic()
            print('Running server/reboot ' + name, flush=True)
            try:
                if name.endswith('-refused'):
                    action = name[:-len('-refused')]
                    result = probe.ssh('/sbin/' + action)
                    if result.returncode != 1 or 'permission denied' not in result.stderr:
                        raise ValueError('unprivileged command did not report permission denied')
                    kernel = probe.ssh('/sbin/' + action + ' --kernel-check')
                    if kernel.returncode != 1 or 'permission denied' not in kernel.stderr:
                        raise ValueError('kernel did not refuse the unprivileged reboot call with EPERM')
                    probe.require(probe.ssh('echo still-running'), 'still-running')
                elif name == 'panic-reset':
                    help_text = subprocess.run(['python3', str(Path(api['__file__']).with_name('metal-run')), 'boot', '--help'], capture_output=True, text=True, check=True).stdout
                    if 'panic-reboot' not in help_text:
                        raise ValueError('boot loop has no panic-reboot fault-test mode')
                    # The fault boot is deliberately fatal, and cannot supply ordinary milestone evidence.
                    console.logout()
                    subprocess.run(['python3', str(Path(api['__file__']).with_name('metal-run')), 'stop', current], check=True)
                    await_end(current)
                    fault = boot_command('boot', '--branch', args.branch or 'main', '--sha', args.sha, '--inject', 'panic-reboot')
                    await_end(fault, 1200)
                    serial = read(fault, 'serial.txt')
                    events = [json.loads(line) for line in read(fault, 'events.jsonl').splitlines()]
                    if 'KERNEL PANIC: reboot fault test' not in serial or not any(e['event'] == 'panic-reset-observed' for e in events):
                        raise ValueError('panic did not print and reset autonomously')
                    fault_run = json.loads(read(fault, 'run.json'))
                    # S22: nonpersistent boots retain their validated image identity in delivery.json.
                    fault_build = json.loads(read(fault, 'delivery.json'))['build']
                    if fault_build.get('commit') != args.sha or fault_run.get('toolsCommit') != args.sha or fault_run.get('kind') != 'fault-test':
                        raise ValueError('panic fault boot has the wrong tools or boot kind')
                    records['panicBoot'] = dict(id=fault, run=fault_run, build=fault_build, serial=serial, events=events)
                    current = boot_command('boot', '--persistent')
                    console = connect()
                else:
                    transition_offset = len(read(current, 'serial.txt'))
                    token = uuid.uuid4().hex
                    home = '/home/' + site['OPERATOR_USER'] + '/.reboot-suite-' + token
                    root = '/var/lib/reboot-suite-' + token
                    resources.append((root, home, '/run/breenix/services.d/reboot-' + token))
                    # The nonce names this case's fixture directories.
                    console.run('sh -c ' + shlex.quote('set -e; mkdir ' + root + '; printf %s ' + token + ' > ' + root + '/data'))
                    probe.require(probe.ssh('set -e; mkdir ' + home + '; printf %s ' + token + ' > ' + home + '/data'))
                    table = '# ' + api['FIXTURE_MARK'] + '\n'
                    for service, needs in [('reboot-base', ''), ('reboot-child', 'needs = reboot-base\n')]:
                        script = "trap 'echo " + service + ' >> ' + root + "/order; exit 0' TERM; echo ready; while :; do sleep 1; done"
                        table += '[' + service + ']\ncommand = /bin/sh -c ' + shlex.quote(script) + '\nuser = root\nrestart = never\n' + needs
                    fixture = '/run/breenix/services.d/reboot-' + token
                    console.run('sh -c ' + shlex.quote('printf %b ' + shlex.quote(table.replace('\\', '\\\\').replace('\n', r'\n')) + ' > ' + fixture))
                    console.run('svc reload')
                    deadline = time.monotonic() + 15
                    while True:
                        state = probe.ssh('cat /run/breenix/services')
                        probe.require(state)
                        _, services = api['parse_state'](state.stdout)
                        if all(services.get(s, {}).get('state') == 'running' for s in ('reboot-base', 'reboot-child')): break
                        if time.monotonic() >= deadline: raise ValueError('shutdown fixtures never started')
                        time.sleep(.2)
                    # Check the on-disk dirty state before shutdown, as well as clean state after it.
                    before = console.run('cat /proc/breenix/filesystems')
                    if not all(re.search(r'^' + m + r' mounted=dirty\r?$', before, re.M) for m in ('root', 'home')):
                        raise ValueError('writable mounts did not clear the on-disk clean state')
                    console.logout()
                    process = subprocess.Popen(probe.common + ['-i', str(identity), '-tt', probe.user + '@' + probe.host,
                                               'sudo /sbin/' + name], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
                    output = b''
                    deadline = time.monotonic() + 25
                    try:
                        while b'[sudo] password for ' not in output:
                            if time.monotonic() >= deadline: raise ValueError('sudo did not ask for the operator password')
                            if select.select([process.stdout], [], [], .2)[0]:
                                chunk = os.read(process.stdout.fileno(), 4096)
                                if not chunk: raise ValueError('sudo ended before authentication')
                                output += chunk
                        process.stdin.write(site['OPERATOR_PASSWORD'].encode() + b'\n'); process.stdin.flush()
                        if name == 'reboot':
                            deadline = time.monotonic() + 650
                            while True:
                                events = [json.loads(line) for line in read(current, 'events.jsonl').splitlines()]
                                if any(e['event'] == 'requested-reboot-ready' and e.get('commit') == args.sha for e in events): break
                                if ended(current) or time.monotonic() >= deadline:
                                    raise ValueError('requested reboot did not return autonomously to SSH')
                                time.sleep(2)
                        else:
                            await_end(current)
                    finally:
                        if process.poll() is None: process.terminate()
                        process.wait(timeout=10)
                    previous = current
                    old = read(previous, 'serial.txt')[transition_offset:]
                    events = [json.loads(line) for line in read(previous, 'events.jsonl').splitlines()]
                    if (name != 'reboot' and read(previous, 'exit-status').strip() != '0') or not any(e['event'] == 'requested-shutdown' and e.get('action') == name for e in events):
                        raise ValueError('boot loop did not record the request as an ordinary boot end')
                    if name == 'reboot' and not any(e['event'] == 'requested-reset-observed' for e in events):
                        raise ValueError('no reset observed after the ACPI request')
                    if not all('[shutdown] ' + m + ' clean read-only' in old for m in ('home', 'root')):
                        raise ValueError('shutdown did not flush and remount both filesystems read-only')
                    if name != 'reboot':
                        current = boot_command('boot', '--persistent')
                    console = connect()
                    fresh = read(current, 'serial.txt')
                    if name == 'reboot':
                        # Only the autonomous return's serial can supply clean readback.
                        fresh = fresh[fresh.rfind('UEFI bootloader started'):]
                        old = old[:old.rfind('UEFI bootloader started')]
                    stops = [old.find('[shutdown] stopping ' + service) for service in ('reboot-child', 'reboot-base')]
                    if not (0 <= stops[0] < stops[1]):
                        raise ValueError('supervisor did not stop the dependent before its provider')
                    if not all('[storage] ' + m + ' prior state=clean' in fresh for m in ('root', 'home')):
                        raise ValueError('next boot did not find both filesystems clean')
                    text = console.run('sha256sum ' + root + '/order ' + root + '/data')
                    verify_root_readback(root, token, text)
                    probe.require(probe.ssh('cat ' + home + '/data'), token)
                    probe.require(probe.ssh('rm -r ' + home))
                    console.run('rm -r ' + root)
                    records['transitions'].append(dict(action=name, before=previous, after=current, token=token,
                                                       beforeSerial=old, afterSerial=fresh, events=events,
                                                       beforeRun=json.loads(read(previous, 'run.json')),
                                                       afterRun=json.loads(read(current, 'run.json')), exit=None if name == 'reboot' else read(previous, 'exit-status').strip(),
                                                       rootReadback=text))
                result, reason = 'PASS', ''
            except Exception as error:
                result, reason = 'FAIL', type(error).__name__ + ': ' + str(error)
                if ended(current):
                    current = boot_command('boot', '--persistent')
                    console = connect()
                for root, home, fixture in resources:
                    console.run('sh -c ' + shlex.quote('rm -f ' + fixture + '; svc reload'))
                    # Inspect the two fixture services after reload requested their removal.
                    for service in ('reboot-child', 'reboot-base'):
                        probe.ssh('svc status ' + service)
                # svc reload waits asynchronously; allow fixture exit before deleting its output directory.
                time.sleep(3)

            elapsed = int((time.monotonic() - started) * 1000)
            records['cases'].append(dict(id='reboot/' + name, result=result, reason=reason, ms=elapsed))
            print(f'SUITE server CASE reboot/{name} {result} ms={elapsed} msg={reason}', flush=True)
        for root, home, fixture in resources:
            console.run('sh -c ' + shlex.quote('set -e; rm -f ' + fixture + '; svc reload; rm -rf ' + root))
            probe.require(probe.ssh('rm -rf ' + home))
        records['boot'] = current
        boot = json.loads(read(current, 'run.json'))
        serial = read(current, 'serial.txt')
        records['consoleGaps'] = [json.loads(line) for line in read(current, 'events.jsonl').splitlines()
                                  if json.loads(line)['event'] == 'console-gap']
        try:
            verdict = api['score'](json.loads(read(current, 'definition/docs/suites/server.json')),
                                   json.loads(read(current, 'definition/docs/efforts/path.json')), records, serial,
                                   boot, args.sha, json.loads(read(current, 'definition/docs/boot-path.json'))['fatal'])
        except ValueError as error:
            verdict = 'FAIL: ' + str(error)
        records['verdict'] = verdict
        save(current, 'server-reboot.json', records)
        console.logout()
        print(verdict, flush=True)
    passed = sum(case['result'] == 'PASS' for case in records['cases'])
    print(f'SUITE server DONE passed={passed} failed={len(cases)-passed} skipped=0 total={len(cases)}', flush=True)
    print('Final reboot evidence boot: ' + current, flush=True)
    return int(passed != len(cases) or not verdict.startswith('PASS:'))
