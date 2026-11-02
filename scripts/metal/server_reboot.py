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
    machine = api['QemuTarget'](args.run_dir, args.boot) if args.target == 'qemu' else api['MetalTarget'](args.boot)
    def read(boot, name):
        return machine.read(name, boot=boot)
    def save(boot, name, value):
        result = machine.select(boot).write(name, value)
        if result.returncode: raise ValueError('cannot save reboot evidence')
    def ended(boot):
        return machine.ended(boot)
    def await_end(boot, seconds=120):
        deadline = time.monotonic() + seconds
        while not ended(boot):
            if time.monotonic() >= deadline: raise ValueError('requested shutdown did not end the boot')
            time.sleep(1)
    site = machine.reboot_site()
    if not all(site.values()): raise ValueError('reboot cases need the operator and console site fields')
    identity = next((p.with_suffix('') for p in (Path.home() / '.ssh').glob('*.pub')
                     if p.read_text().split()[:2] == site['OPERATOR_SSH_PUBLIC_KEY'].split()[:2]
                     and p.with_suffix('').is_file()), None)
    if identity is None: raise ValueError('no private key matches the site key')
    current = args.boot
    records = dict(boot=current, commit=args.sha, milestone='server/reboot', cases=[], cleanup=True,
                   consoleGaps=[], transitions=[])
    with tempfile.TemporaryDirectory(prefix='server-reboot-') as scratch:
        probe = api['Probe'](site['SERVER_SSH_HOST'], site['OPERATOR_USER'], identity, Path(scratch),
                             **(dict(port=int(site['SERVER_SSH_PORT'])) if args.target == 'qemu' else {}))
        def connect():
            nonlocal machine
            machine = machine.select(current)
            deadline = time.monotonic() + 600
            while True:
                boot = json.loads(read(current, 'run.json'))
                if ended(current): raise ValueError('boot ended before the reboot case could connect')
                capture = machine.read('serial.txt', required=False)
                if capture is not None and 'bsshd: listening on 0.0.0.0:22' in capture: break
                if time.monotonic() >= deadline: raise ValueError('persistent boot never reached SSH')
                time.sleep(2)
            if (boot.get('bootedCommit'), boot.get('toolsCommit'), boot.get('mode'), boot.get('evidenceEligible')) != (args.sha, args.sha, 'persistent', True):
                raise ValueError('reboot evidence needs the exact kernel and tools commits')
            if not machine.holds_lease():
                raise ValueError('case boot has no machine lease')
            if args.target == 'qemu':
                probe.port = int(machine.reboot_site()['SERVER_SSH_PORT'])
                probe.common[probe.common.index('-p') + 1] = str(probe.port)
            console = machine.console(site['CONSOLE_USER'], site['CONSOLE_PASSWORD'])
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
                elif name == 'refusal-recovery':
                    # Refuse before the filesystem barrier using the real protected-file
                    # validation, after the services and existing sessions have stopped.
                    offset = len(read(current, 'serial.txt'))
                    state = probe.ssh('cat /run/breenix/services')
                    probe.require(state)
                    _, before = api['parse_state'](state.stdout)
                    console.run('chmod 600 /etc/breenix/power-token')
                    console.logout()
                    process = subprocess.Popen(probe.common + ['-i', str(identity), '-tt', probe.user + '@' + probe.host,
                                               'sudo /sbin/reboot'], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
                    output = b''
                    deadline = time.monotonic() + 25
                    try:
                        while b'[sudo] password for ' not in output:
                            if time.monotonic() >= deadline: raise ValueError('refusal sudo prompt missing')
                            if select.select([process.stdout], [], [], .2)[0]:
                                chunk = os.read(process.stdout.fileno(), 4096)
                                if not chunk: raise ValueError('refusal sudo ended before authentication')
                                output += chunk
                        process.stdin.write(site['OPERATOR_PASSWORD'].encode() + b'\n'); process.stdin.flush()
                        deadline = time.monotonic() + 120
                        while '[shutdown] refused; resuming services' not in read(current, 'serial.txt')[offset:]:
                            if ended(current) or time.monotonic() >= deadline:
                                raise ValueError('shutdown refusal did not resume services')
                            time.sleep(1)
                    finally:
                        if process.poll() is None: process.terminate()
                        process.wait(timeout=10)
                    console = connect()
                    console.run('chmod 400 /etc/breenix/power-token')
                    probe.require(probe.ssh('echo refusal-reconnected'), 'refusal-reconnected')
                    state = probe.ssh('cat /run/breenix/services')
                    probe.require(state)
                    _, after = api['parse_state'](state.stdout)
                    if after['bsshd']['state'] != 'running' or after['bsshd']['starts'] <= before['bsshd']['starts']:
                        raise ValueError('refusal did not restart bsshd')
                    console.run('svc reload')
                    records['refusalRecovery'] = dict(serial=read(current, 'serial.txt')[offset:], before=before, after=after)
                elif name == 'panic-reset':
                    # The fault boot cannot supply ordinary milestone evidence.
                    console.logout()
                    machine.stop_for_fault(site['CONSOLE_USER'], site['CONSOLE_PASSWORD'])
                    await_end(current)
                    fault = machine.start_fault(args.branch, args.sha)
                    await_end(fault, 1200)
                    serial = read(fault, 'serial.txt')
                    events = [json.loads(line) for line in read(fault, 'events.jsonl').splitlines()]
                    if 'KERNEL PANIC: reboot fault test' not in serial or not any(e['event'] == 'panic-reset-observed' for e in events):
                        raise ValueError('panic did not print and reset autonomously')
                    fault_run = json.loads(read(fault, 'run.json'))
                    # S22: nonpersistent boots retain their validated image identity in delivery.json.
                    fault_build = machine.select(fault).fault_build()
                    if fault_build.get('commit') != args.sha or fault_run.get('toolsCommit') != args.sha or fault_run.get('kind') != 'fault-test' or (args.target == 'qemu' and (fault_run.get('bootedCommit') != args.sha or not fault_run.get('evidenceEligible'))):
                        raise ValueError('panic fault boot has the wrong tools or boot kind')
                    records['panicBoot'] = dict(id=fault, run=fault_run, build=fault_build, serial=serial, events=events)
                    current = machine.start_persistent()
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
                                successor = machine.reboot_successor(args.sha)
                                if successor is not None: break
                                if time.monotonic() >= deadline:
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
                        current = machine.start_persistent()
                    else:
                        current = successor
                    console = connect()
                    fresh = read(current, 'serial.txt')
                    if name == 'reboot':
                        # Only the autonomous return's serial can supply clean readback.
                        marker = machine.boot_start_marker
                        if marker not in fresh: raise ValueError('successor has no boot-start marker')
                        fresh = fresh[fresh.rfind(marker):]
                        if args.target == 'metal': old = old[:old.rfind(marker)]
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
                    current = machine.start_persistent()
                    console = connect()
                if name == 'refusal-recovery':
                    console.run('chmod 400 /etc/breenix/power-token')
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
    if args.target == 'qemu':
        file_vigil(api, args, machine, records, verdict, passed)
    print('Final reboot evidence boot: ' + current, flush=True)
    return int(passed != len(cases) or not verdict.startswith('PASS:'))


def score_qemu(records, boot, sha, fatal_patterns):
    """Judge ARM64 QEMU transitions independently of metal's session model."""
    recovery = records.get('refusalRecovery', {})
    if ('[shutdown] kernel refused transition: -13' not in recovery.get('serial', '') or
            '[shutdown] refused; resuming services' not in recovery.get('serial', '')):
        raise ValueError('no kernel refusal and service recovery evidence')
    transitions = records.get('transitions', [])
    if [t['action'] for t in transitions] != ['reboot', 'poweroff']:
        raise ValueError('reboot needs both completed shutdown transitions')
    if not boot.get('runId') or not boot.get('qemuProcess'):
        raise ValueError('QEMU evidence has no run or process identity')
    for transition in transitions:
        before, after = transition['beforeRun'], transition['afterRun']
        for field, run in (('before', before), ('after', after)):
            if (run.get('id') != transition[field] or run.get('target') != 'qemu' or
                    run.get('runId') != boot['runId'] or not run.get('qemuProcess') or
                    (run.get('bootedCommit'), run.get('toolsCommit'), run.get('mode'), run.get('evidenceEligible')) !=
                    (sha, sha, 'persistent', True)):
                raise ValueError('transition needs this kernel and tools in this QEMU run')
        events = transition['events']
        action = transition['action']
        receipts = [e for e in events if e.get('event') == 'requested-shutdown']
        if len(receipts) != 1 or receipts[0].get('action') != action:
            raise ValueError('missing authenticated shutdown receipt')
        if 'bsshd: listening on 0.0.0.0:22' not in transition['afterSerial']:
            raise ValueError('successor did not reach SSH at this commit')
        if action == 'reboot':
            resets = [e for e in events if e.get('event') == 'requested-reset-observed']
            raw = [e for e in events if e.get('event') == 'RESET']
            if (transition['before'] == transition['after'] or len(resets) != 1 or
                    resets[0].get('successor') != transition['after'] or
                    before['qemuProcess'] != after['qemuProcess'] or len(raw) != 1 or
                    raw[0].get('data', {}).get('reason') != 'guest-reset' or
                    events.index(receipts[0]) > events.index(raw[0]) or
                    events.index(raw[0]) > events.index(resets[0])):
                raise ValueError('reboot has no authenticated successor in the same QEMU process')
        elif (transition['exit'] != '0' or before['qemuProcess'] == after['qemuProcess'] or
              not any(e.get('event') == 'SHUTDOWN' and e.get('data', {}).get('reason') == 'guest-shutdown'
                      for e in events[events.index(receipts[0]) + 1:])):
            raise ValueError('poweroff did not end ordinarily before relaunch')
        for mount in ('home', 'root'):
            if ('[shutdown] ' + mount + ' clean read-only' not in transition['beforeSerial'] or
                    '[storage] ' + mount + ' prior state=clean' not in transition['afterSerial']):
                raise ValueError('transition has no durable clean filesystem evidence')
        if any(re.search(pattern, transition[field]) for pattern in fatal_patterns + [r'KERNEL PANIC:']
               for field in ('beforeSerial', 'afterSerial')):
            raise ValueError('fatal output in a clean shutdown transition')
    panic = records.get('panicBoot', {})
    fault = panic.get('run', {})
    if (panic.get('build', {}).get('commit') != sha or fault.get('bootedCommit') != sha or
            fault.get('toolsCommit') != sha or fault.get('kind') != 'fault-test' or
            fault.get('target') != 'qemu' or fault.get('evidenceEligible') is not True or
            fault.get('runId') != boot['runId'] or not fault.get('qemuProcess') or
            panic.get('id') != fault.get('id') or
            'KERNEL PANIC: reboot fault test' not in panic.get('serial', '') or
            not any(e.get('event') == 'panic-reset-observed' for e in panic.get('events', [])) or
            not any(e.get('event') == 'SHUTDOWN' and e.get('data', {}).get('reason') == 'guest-reset'
                    for e in panic.get('events', []))):
        raise ValueError('panic reset has no matching fault-test boot evidence')


def file_vigil(api, args, machine, records, verdict, passed):
    lines = ['SUITE server START cases=' + str(len(api['REBOOT']))]
    lines += [f"SUITE server CASE {case['id']} {case['result']} ms={case['ms']} msg={case['reason']}"
              for case in records['cases']]
    lines += [f"SUITE server DONE passed={passed} failed={len(api['REBOOT'])-passed} skipped=0 total={len(api['REBOOT'])}"]
    suite_file = machine.directory / 'reboot-suite.txt'
    suite_file.write_text('\n'.join(lines) + '\n')
    status = int(passed != len(api['REBOOT']) or not verdict.startswith('PASS:'))
    subprocess.run([str(Path(api['__file__']).resolve().parent.parent / 'vigil-record.sh'),
                    'record', 'qemu', 'persistent', 'server', str(suite_file),
                    str(machine.directory / 'serial.txt'), (args.run_dir / 'qemu-profile').read_text().strip(),
                    time.strftime('%Y%m%dT%H%M%SZ', time.gmtime()) + '-qemu-' + uuid.uuid4().hex,
                    args.sha, (machine.directory / 'started').read_text().strip(), str(time.time()), str(status)],
                   check=True, env=dict(os.environ, BREENIX_VIGIL_ARCH='aarch64'))
