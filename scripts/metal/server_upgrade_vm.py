"""ARM64 QEMU upgrade transactions; acceptance uses the VM runner and Vigil.

The metal upgrade driver and the common assertions remain unchanged. Importing
this module never builds or boots anything; run() is the category entry point.
"""
import json
import math
import os
from pathlib import Path
import re
import runpy
import secrets
import socket
import subprocess
import tempfile
import time
import uuid

ROOT = Path(__file__).resolve().parents[2]
UpgradeSuite = runpy.run_path(str(Path(__file__).with_name('upgrade_suite.py')))['UpgradeSuite']


def probe_upgrade(machine, probe, site, host, generation, fingerprint):
    """One S21 attempt, bounded by this launch's recorded start, including SSH."""
    power = float((machine.directory / 'started').read_text())
    receipt = dict(boot=machine.boot, powerOn=power, passed=False, sshAttempts=0)
    try:
        if not math.isfinite(power) or not power < time.time() < power + 600:
            raise ValueError('outside the launch power-on deadline')
        if not machine.holds_lease(): raise ValueError('upgrade probe lost its live VM lease')
        if 'StrictHostKeyChecking=yes' not in probe.common or not probe.known.is_file():
            raise ValueError('upgrade probe has no console-pinned strict host key')

        def ssh(command):
            remaining = power + 600 - time.time()
            if remaining <= 0: raise ValueError('upgrade probe deadline expired')
            receipt['sshAttempts'] += 1
            return probe.ssh(command, timeout=min(15, remaining))

        probe.require(ssh('id -u'), site['OPERATOR_UID'])
        observed = ssh('cat /usr/share/factory/generation')
        probe.require(observed)
        if observed.stdout.strip() != generation: raise ValueError('probe booted another generation')
        state = ssh('cat /run/breenix/services')
        probe.require(state)
        _, services = host['parse_state'](state.stdout)
        for name in host['Services'].REQUIRED:
            row = services.get(name, {})
            if row.get('starts', 0) < 1 or (name != 'ntp' and (row.get('state') != 'running' or not row.get('pid'))):
                raise ValueError('service not started: ' + name)
        completed = time.time()
        if not power < completed <= power + 600: raise ValueError('upgrade probe deadline expired')
        receipt.update(authenticated=True, strictHostKey=True, servicesStarted=True,
                       generation=observed.stdout.strip(), fingerprint=fingerprint,
                       serviceState=state.stdout, completedAt=completed, passed=True)
    except Exception as error:
        receipt.update(error=type(error).__name__ + ': ' + str(error), completedAt=time.time())
    return receipt


class VmOps:
    """Run u3-u5 operations locally; only launches take the Mac boot slot."""
    def __init__(self, machine, sha):
        self.machine, self.run, self.sha = machine, machine.run, sha
        self.children = []

    def command(self, operation, *arguments):
        result = subprocess.run([str(ROOT / 'scripts/vm-persistent.sh'), 'qemu', operation,
                                 '--run-dir', str(self.run), *arguments],
                                text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                                timeout=15000)
        (self.run / (operation + '-upgrade.log')).write_text(result.stdout)
        if result.returncode: raise ValueError('VM ' + operation + ' failed; see its upgrade log')

    def references(self):
        record = json.loads((self.run / 'slots.json').read_text()) if (self.run / 'slots.json').exists() else {}
        return {name.upper(): record[name]['userland'] if record.get(name) else
                ('flat' if name == 'current' else 'empty') for name in ('current', 'previous', 'next')}

    def stage(self, inject=None):
        self.command('stage-candidate', *(['--inject', inject] if inject else []))
        candidate = json.loads((self.run / 'candidate.json').read_text())
        if candidate.get('commit') != self.sha or candidate.get('injectedFault') != inject:
            raise ValueError('candidate seal does not match this commit and fault')
        return candidate

    def install(self):
        previous = (self.run / 'current').read_text().strip()
        self.command('install')
        boot = (self.run / 'current').read_text().strip()
        if boot == previous: raise ValueError('installer produced no new boot record')
        self.machine = self.machine.select(boot)
        if '[persistent] install complete:' not in self.machine.read('serial.txt'):
            raise ValueError('installer has no completion transcript')
        return self.machine

    def boot(self, slot):
        previous = (self.run / 'current').read_text().strip()
        mode = (self.run / 'mode').read_text().strip()
        with (self.run / 'upgrade-launcher.log').open('ab') as log:
            child = subprocess.Popen([str(ROOT / 'scripts/vm-persistent.sh'), 'qemu', 'boot',
                                      '--run-dir', str(self.run), '--mode', mode, '--slot', slot],
                                     stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
        self.children.append(child)
        deadline = time.monotonic() + 1200
        try:
            while time.monotonic() < deadline:
                current = (self.run / 'current').read_text().strip()
                if current != previous:
                    self.machine = self.machine.select(current)
                    return self.machine
                if child.poll() is not None: raise ValueError('upgrade launch failed; see upgrade-launcher.log')
                time.sleep(.2)
            raise ValueError('upgrade launch produced no boot record')
        except Exception:
            # This Popen handle owns the lease supervisor; its termination
            # stops its own launch, including one still queued for mac-boot.
            if child.poll() is None: child.terminate()
            child.wait(timeout=180)
            current = (self.run / 'current').read_text().strip()
            if current != previous: self.machine = self.machine.select(current)
            raise

    def transition(self, operation):
        if operation not in ('promote', 'rollback'): raise ValueError('invalid upgrade transition')
        self.command(operation)

    def stop(self, machine, console=None, force=False):
        if not machine.ended(machine.boot):
            if not machine.holds_lease(): raise ValueError('cannot stop a VM without its lease')
            if force:
                # stop deletes the disposable directory; quit only our leased
                # QEMU here, retaining its disks and failed-probe evidence.
                with socket.socket(socket.AF_UNIX) as stream:
                    stream.settimeout(10)
                    stream.connect(str(self.run / 'qmp.sock'))
                    with stream.makefile('rwb', buffering=0) as source:
                        json.loads(source.readline())
                        for command in ('qmp_capabilities', 'quit'):
                            source.write((json.dumps(dict(execute=command)) + '\n').encode())
                            while True:
                                reply = json.loads(source.readline())
                                if 'error' in reply: raise ValueError(str(reply['error']))
                                if 'return' in reply: break
            else:
                site = machine.reboot_site()
                console = console or machine.console(site['CONSOLE_USER'], site['CONSOLE_PASSWORD'])
                console.login()
                console.type_line('/sbin/poweroff')
        deadline = time.monotonic() + 180
        while not machine.ended(machine.boot) or (self.run / 'boot.lock').exists():
            if time.monotonic() >= deadline: raise ValueError('upgrade QEMU did not finish stopping')
            time.sleep(.2)
        for child in self.children:
            child.wait(timeout=10)
        self.children.clear()
        if not force:
            events = [json.loads(line) for line in machine.read('events.jsonl').splitlines()]
            requested = False
            clean = False
            for event in events:
                if event.get('event') == 'requested-shutdown': requested = event.get('action') == 'poweroff'
                if requested and event.get('event') == 'SHUTDOWN' and event.get('data', {}).get('reason') == 'guest-shutdown':
                    clean = True
            if not clean: raise ValueError('upgrade stop has no authenticated guest poweroff')
            runpy.run_path(str(ROOT / 'scripts/vm-artifacts.py'))['continuation'](self.run)


class QemuUpgrade(UpgradeSuite):
    def __init__(self, args, host, ops, identity=None):
        self.args, self.host, self.ops = args, dict(host), ops
        self.host['upgrade_cases'] = runpy.run_path(str(Path(__file__).with_name('upgrade_cases.py')))
        self.machine = ops.machine
        self.site = self.machine.reboot_site()
        if self.site['CONSOLE_USER'] != 'root': raise ValueError('upgrade sentinels require root on the console')
        self.identity = identity or next((p.with_suffix('') for p in (Path.home() / '.ssh').glob('*.pub')
            if p.read_text().split()[:2] == self.site['OPERATOR_SSH_PUBLIC_KEY'].split()[:2]
            and p.with_suffix('').is_file()), None)
        if self.identity is None: raise ValueError('upgrade needs the site operator private key')
        self.active = self.anchor = args.boot
        self.boots = [args.boot]
        self.machines = {args.boot: self.machine}
        self.baseline, self.console, self.probe = None, None, None
        self.results, self.probes = {}, []
        self.scratch = tempfile.TemporaryDirectory(prefix='vm-upgrade-')
        nonce = secrets.token_hex(16)
        self.root_file, self.home_file = '/root/.breenix-upgrade-' + nonce, '.breenix-upgrade-' + nonce
        self.token = 'remote-upgrade-' + nonce
        self.known, self.fingerprint = None, None

    def read(self, boot, name, optional=False):
        return self.machines[boot].read(name, required=not optional) or ''

    def select(self, machine):
        self.machine, self.active = machine, machine.boot
        self.machines[machine.boot] = machine
        if machine.boot not in self.boots: self.boots.append(machine.boot)
        record = machine.identity()
        if (record.get('target'), record.get('mode'), record.get('bootedCommit'), record.get('toolsCommit')) != (
                'qemu', 'persistent', self.args.sha, self.args.sha):
            raise ValueError('upgrade VM kernel and tools must match this category head')

    def new_probe(self):
        self.site = self.machine.reboot_site()
        directory = Path(self.scratch.name) / (self.active + '-' + secrets.token_hex(4))
        directory.mkdir()
        probe = self.host['Probe'](self.site['SERVER_SSH_HOST'], self.site['OPERATOR_USER'], self.identity,
                                   directory, port=int(self.site['SERVER_SSH_PORT']))
        self.probes.append(probe)
        if self.known:
            pin = self.known.split()
            if len(pin) != 3 or pin[1] != 'ssh-ed25519': raise ValueError('upgrade needs a unique Ed25519 pin')
            endpoint = '[' + self.site['SERVER_SSH_HOST'] + ']:' + str(self.site['SERVER_SSH_PORT'])
            probe.known.write_text(endpoint + ' ' + ' '.join(pin[1:]) + '\n')
        return probe

    def connect(self):
        self.select(self.machine)
        power = float((self.machine.directory / 'started').read_text())
        while 'Breenix login: ' not in self.machine.read('serial.txt'):
            if time.time() >= power + 600 or self.machine.ended(self.active):
                raise ValueError('persistent VM never reached login within its launch deadline')
            time.sleep(1)
        if not self.machine.identity().get('evidenceEligible') or not self.machine.holds_lease():
            raise ValueError('upgrade baseline needs an eligible live VM')
        self.console = self.machine.console(self.site['CONSOLE_USER'], self.site['CONSOLE_PASSWORD'])
        log = self.console.service_log('bsshd')
        fingerprint = re.search(r'host key ED25519 (SHA256:[A-Za-z0-9+/]+)', log)
        if not fingerprint: raise ValueError('console has no protected host-key fingerprint')
        if self.fingerprint and fingerprint[1] != self.fingerprint: raise ValueError('host-key fingerprint changed')
        self.fingerprint = fingerprint[1]
        self.probe = self.new_probe()
        if self.known is None:
            self.probe.run_case('host-key', log)
            self.known = self.probe.known.read_text()
        self.probe.require(self.probe.ssh('id -u'), self.site['OPERATOR_UID'])

    def verify(self, generation):
        probe = self.new_probe()
        attempts = 0
        while True:
            receipt = probe_upgrade(self.machine, probe, self.site, self.host, generation, self.fingerprint)
            attempts += receipt['sshAttempts']
            receipt['sshAttempts'] = attempts
            self.machine.write('ssh-verification-progress.json', receipt)
            if receipt['passed'] or self.machine.ended(self.active) or time.time() >= receipt['powerOn'] + 600:
                break
            time.sleep(min(2, max(0, receipt['powerOn'] + 600 - time.time())))
        self.machine.write('ssh-verification.json', receipt)
        return receipt

    def stop(self, force=False):
        if self.active is None: return
        self.ops.stop(self.machine, self.console, force=force)
        self.active, self.console = None, None

    def install(self, inject=None):
        refs = self.ops.references()
        candidate = self.ops.stage(inject)
        self.select(self.ops.install())
        result = dict(install=self.active, toolsCommit=self.args.sha, candidateGeneration=candidate['generation'],
                      previousGeneration=refs['CURRENT'], recoveredReferences=refs)
        self.active = None  # install returns only after the launcher exits
        return result

    def prepare(self):
        self.connect()
        self.stop()
        seeds = self.results['seedInstalls'] = []
        for _ in range(2):
            result = self.install()
            seeds.append(result)
            self.select(self.ops.boot('next'))
            result['verification'] = self.verify(result['candidateGeneration'])
            if not result['verification']['passed']: raise ValueError('seed generation failed S21 verification')
            self.stop()
            self.ops.transition('promote')
        self.select(self.ops.boot('current'))
        super().prepare()

    def attempt(self, broken, bootable=False):
        name = 'probe-failure' if bootable else 'broken' if broken else 'good'
        before = self.snapshot()
        self.stop()
        result = self.install('upgrade-service-down' if bootable else 'upgrade-no-ssh' if broken else None)
        self.results[name] = result
        try:
            if result['previousGeneration'] != before['generation']:
                raise ValueError('upgrade did not retain the original paired generation')
            try:
                self.select(self.ops.boot('next'))
                result['verification'] = self.verify(result['candidateGeneration'])
            except Exception as error:
                result['launchError'] = type(error).__name__ + ': ' + str(error)
                result['verification'] = dict(passed=False, sshAttempts=0, error=result['launchError'])
                if self.ops.machine.boot != result['install']:
                    self.select(self.ops.machine)
            serial = self.machine.read('serial.txt') if self.active is not None else ''
            result['candidateBooted'] = 'Breenix ARM64 Kernel Starting' in serial and 'Breenix login: ' in serial
            passed = result['verification']['passed']
            # A failed, bootable SSH candidate can still power off via its root
            # console. An invalid kernel must be stopped via this run's QMP.
            self.stop(force=not passed and not result['candidateBooted'])
            if passed:
                self.ops.transition('promote')
                result['outcome'] = 'verified'
                self.select(self.ops.boot('current'))
            else:
                self.ops.transition('rollback')
                self.select(self.ops.boot('current'))
                result['rollback'] = self.verify(result['previousGeneration'])
                if not result['rollback']['passed']: raise ValueError('previous generation failed S21 recovery')
                result['outcome'] = 'rolled-back'
            self.connect()
            after = self.snapshot()
            expected = result['previousGeneration'] if not passed else result['candidateGeneration']
            if after['generation'] != expected: raise ValueError('current slot selected another paired userland')
            result['observations'] = [before, after]
            if result.get('launchError'):
                raise ValueError('candidate launch failed; current recovered: ' + result['launchError'])
            self.host['upgrade_cases']['upgrade'](result, broken, self.baseline, bootable=bootable)
        finally:
            self.machines[result['install']].write('upgrade-result.json', result)

    def generation_proofs(self):
        """Execute the existing installer harness in a disposable local Linux container.

        The directory-alias case needs Linux bind mounts, including on a Mac.
        No guest boot or remote metal host is involved.
        """
        if 'generationProof' in self.results: return
        files = ('userspace/programs/src/persistent_install.rs', 'libs/libbreenix/src/layout.rs',
                 'libs/libbreenix/src/layout.tsv', 'kernel/src/fs/ext2/generation.rs',
                 'scripts/metal/efi-stub/src/slot_transaction.rs')
        tests = (
            ('interruption_enospc_and_io_error_at_each_operation_keep_current_bootable', ('enospc', 'io', 'interruption')),
            ('promotion_and_rollback_recover_paired_trees_byte_for_byte', ('paired-rollback',)),
            ('reclamation_retains_recovered_slots_and_shared_data_after_each_failure', ('reclamation',)),
            ('reclamation_preserves_directory_aliases_and_reclaims_plain_scratch', ('directory-aliases',)))
        with tempfile.TemporaryDirectory(prefix='vm-upgrade-tests-') as scratch:
            base = Path(scratch)
            for name in files:
                output = base / name
                output.parent.mkdir(parents=True, exist_ok=True)
                output.write_bytes(subprocess.check_output(['git', '-C', str(ROOT), 'show', self.args.sha + ':' + name]))
            # One container compiles once and executes each named case once.
            program = ['set -eu', 'rustc --edition=2021 --test /src/' + files[0] + ' -o /tmp/installer-tests 2>/src/compile.log',
                       'test ! -s /src/compile.log']
            for index, (test, _) in enumerate(tests):
                prefix = 'unshare --mount --propagation private -- ' if index == 3 else ''
                program.append('set +e; ' + prefix + '/tmp/installer-tests --exact tests::' + test +
                               ' --nocapture >/src/test-' + str(index) + '.log 2>&1; '
                               'echo $? >/src/test-' + str(index) + '.status; set -e')
            result = subprocess.run(['docker', 'run', '--rm', '--network', 'none', '--cap-add=SYS_ADMIN',
                                     '--security-opt', 'apparmor=unconfined', '-v', scratch + ':/src',
                                     os.environ.get('BREENIX_UPGRADE_TEST_IMAGE', 'rust:1.90'),
                                     'sh', '-c', '\n'.join(program)], capture_output=True, text=True, timeout=1200)
            if result.returncode:
                compile_log = (base / 'compile.log').read_text() if (base / 'compile.log').exists() else ''
                raise ValueError('local installer harness did not compile cleanly: ' + compile_log + result.stderr)
            proof = {}
            for index, (_, names) in enumerate(tests):
                output = (base / ('test-' + str(index) + '.log')).read_text()
                receipt = dict(exit=int((base / ('test-' + str(index) + '.status')).read_text()),
                               executed='1 passed' in output, stdout=output, stderr='', commit=self.args.sha)
                for name in names: proof[name] = receipt
            self.results['generationProof'] = proof

    def cleanup(self):
        try:
            super().cleanup()
        finally:
            for probe in self.probes: probe.cleanup()


def run(api, args):
    """Score ARM64 QEMU records with the existing remote-upgrade assertions."""
    machine = api['QemuTarget'](args.run_dir, args.boot)
    target = QemuUpgrade(args, api, VmOps(machine, args.sha))
    manifest = json.loads(machine.read('definition/docs/suites/server.json'))
    api['validate_manifest'](manifest)
    effort = json.loads(machine.read('definition/docs/efforts/path.json'))
    fatal = json.loads(machine.read('definition/docs/boot-path.json'))['fatal'] + ['KERNEL PANIC:']
    records = dict(boot=args.boot, commit=args.sha, milestone='server/remote-upgrade', cases=[], cleanup=False, consoleGaps=[])
    captures, lines = [], []
    def emit(line):
        lines.append(line)
        print(line, flush=True)
    count = len(api['REMOTE_UPGRADE'])
    try:
        target.prepare()
        records['baseline'] = target.baseline
        emit(f'SUITE server START cases={count}')
        for name, _ in api['REMOTE_UPGRADE']:
            started = time.monotonic()
            try:
                target.run_case(name)
                result, reason = 'PASS', ''
            except Exception as error:
                result, reason = 'FAIL', type(error).__name__ + ': ' + str(error)
            case = dict(id='remote-upgrade/' + name, result=result, reason=reason,
                        ms=int((time.monotonic() - started) * 1000))
            records['cases'].append(case)
            emit(f"SUITE server CASE {case['id']} {result} ms={case['ms']} msg={reason}")
    finally:
        try:
            target.cleanup()
            for boot in target.boots:
                capture = target.read(boot, 'serial.txt')
                events = [json.loads(line) for line in target.read(boot, 'events.jsonl').splitlines()]
                records['consoleGaps'].extend(dict(e, boot=boot) for e in events if e.get('event') == 'console-gap')
                if any(re.search(pattern, capture) for pattern in fatal): raise ValueError('fatal output in ' + boot)
                captures.append(capture)
            records['cleanup'] = True
        except Exception as error:
            records['cleanupError'] = str(error)
        records.update(target.results)
        machine.write('server-remote-upgrade.json', records)
    passed = sum(c['result'] == 'PASS' for c in records['cases'])
    emit(f'SUITE server DONE passed={passed} failed={count-passed} skipped=0 total={count}')
    try:
        verdict = api['score'](manifest, effort, records, '\n'.join(captures), machine.identity(), args.sha, fatal)
        status = 0
    except ValueError as error:
        verdict, status = 'FAIL: ' + str(error), 1
    records['verdict'] = verdict
    machine.write('server-remote-upgrade.json', records)
    suite_file = machine.directory / 'upgrade-suite.txt'
    suite_file.write_text('\n'.join(lines) + '\n')
    subprocess.run([str(ROOT / 'scripts/vigil-record.sh'), 'record', 'qemu', 'persistent', 'server',
                    str(suite_file), str(machine.directory / 'serial.txt'),
                    (args.run_dir / 'qemu-profile').read_text().strip(),
                    time.strftime('%Y%m%dT%H%M%SZ', time.gmtime()) + '-qemu-' + uuid.uuid4().hex,
                    args.sha, (machine.directory / 'started').read_text().strip(), str(time.time()), str(status)],
                   check=True, env=dict(os.environ, BREENIX_VIGIL_ARCH='aarch64'))
    print(verdict, flush=True)
    return status
