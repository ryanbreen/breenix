"""Execute the server remote-upgrade category on the boot host through metal-run."""
import hashlib
import json
from pathlib import Path
import re
import runpy
import shlex
import tempfile
import time


class UpgradeSuite:
    def __init__(self, args, host):
        self.args, self.host = args, host
        self.runner = runpy.run_path(str(Path(host['__file__']).with_name('metal-run')))
        self.env = self.runner['host_env']()
        self.root = self.runner['STATE'] + '/boots/'
        self.active = args.boot
        self.anchor = args.boot
        self.boots = [args.boot]
        self.baseline = None
        self.results = {}
        self.console = None
        self.probe = None
        self.scratch = tempfile.TemporaryDirectory(prefix='remote-upgrade-')
        site = self.remote('python3', '-c', 'import sys,json;sys.path.insert(0,"/opt/breenix-metal");'
                           'import metal_lib;s=metal_lib.load_site();print(json.dumps({k:s.get(k) for k in '
                           + repr(('SERVER_SSH_HOST', 'OPERATOR_USER', 'OPERATOR_UID', 'OPERATOR_SSH_PUBLIC_KEY', 'CONSOLE_USER', 'CONSOLE_PASSWORD')) + '}))')
        self.site = json.loads(site.stdout)
        self.identity = next((p.with_suffix('') for p in (Path.home() / '.ssh').glob('*.pub')
                              if p.read_text().split()[:2] == self.site['OPERATOR_SSH_PUBLIC_KEY'].split()[:2]
                              and p.with_suffix('').is_file()), None)
        if self.identity is None: raise ValueError('boot host lacks the operator key')
        nonce = __import__('os').urandom(16).hex()
        self.root_file = '/root/.breenix-upgrade-' + nonce
        self.home_file = '.breenix-upgrade-' + nonce
        self.token = 'remote-upgrade-' + nonce

    def remote(self, *cmd, **kw):
        return self.runner['in_metal'](self.env, *cmd, capture_output=True, text=True, **kw)

    def read(self, boot, name, optional=False):
        result = self.remote('cat', self.root + boot + '/' + name)
        if result.returncode and not optional: raise ValueError('missing boot receipt: ' + name)
        return result.stdout if result.returncode == 0 else ''

    def launch(self, arguments):
        result = self.host['command']([__import__('sys').executable,
                                      str(Path(self.host['__file__']).with_name('metal-run')), *arguments], timeout=15000)
        if result.returncode: raise ValueError('metal-run refused: ' + result.stderr[-500:])
        for line in result.stdout.splitlines():
            try: value = json.loads(line)
            except ValueError: continue
            if 'id' in value:
                self.boots.append(value['id'])
                return value['id']
        raise ValueError('metal-run returned no boot ID')

    def wait(self, boot, name, timeout=15000):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            value = self.read(boot, name, optional=True)
            if value: return value
            status = self.read(boot, 'exit-status', optional=True).strip()
            if status and status != '0': raise ValueError('boot failed before ' + name + ': ' + status)
            time.sleep(2)
        raise ValueError('boot did not produce ' + name)

    def connect(self):
        deadline = time.monotonic() + 600
        while time.monotonic() < deadline:
            serial = self.read(self.active, 'serial.txt', optional=True)
            boot = json.loads(self.read(self.active, 'run.json'))
            if boot.get('bootedCommit') == self.args.sha and 'Breenix login: ' in serial:
                break
            if self.read(self.active, 'exit-status', optional=True): raise ValueError('persistent boot ended')
            time.sleep(2)
        else: raise ValueError('persistent boot never reached login')
        if boot.get('toolsCommit') != self.args.sha or not boot.get('evidenceEligible'):
            raise ValueError('boot kernel and tools must match the category head')
        lease = json.loads(self.remote('cat', self.runner['STATE'] + '/lease.json').stdout)
        if lease.get('id') != self.active: raise ValueError('persistent boot has no lease')
        self.console = self.host['Console'](lambda *cmd, **kw: self.runner['in_metal'](self.env, *cmd, **kw),
                                           self.active, self.root + self.active,
                                           self.site['CONSOLE_USER'], self.site['CONSOLE_PASSWORD'])
        log = self.console.service_log('bsshd')
        self.fingerprint = re.search(r'host key ED25519 (SHA256:[A-Za-z0-9+/]+)', log)[1]
        directory = Path(self.scratch.name) / self.active
        directory.mkdir()
        self.probe = self.host['Probe'](self.site['SERVER_SSH_HOST'], self.site['OPERATOR_USER'], self.identity, directory)
        if self.baseline:
            if self.fingerprint != self.baseline['fingerprint']: raise ValueError('host-key fingerprint changed')
            self.probe.known.write_text(self.known)
        else:
            self.probe.run_case('host-key', log)
            self.known = self.probe.known.read_text()
        self.probe.require(self.probe.ssh('id -u'), self.site['OPERATOR_UID'])

    def snapshot(self):
        root = self.console.run('sha256sum ' + self.root_file)
        matches = re.findall(r'(?m)^([a-f0-9]{64}) +'+re.escape(self.root_file)+r'\r?$', root)
        home = self.probe.ssh('sha256sum "$HOME/' + self.home_file + '"')
        self.probe.require(home)
        if len(matches) != 1 or not re.match(r'^[a-f0-9]{64} ', home.stdout):
            raise ValueError('persistent sentinels could not be read')
        expected = hashlib.sha256((self.token + '\n').encode()).hexdigest()
        if matches[0] != expected or home.stdout.split()[0] != expected:
            raise ValueError('persistent sentinel bytes changed')
        generation = self.probe.ssh('cat /usr/share/factory/generation')
        self.probe.require(generation)
        return dict(commit=self.args.sha, fingerprint=self.fingerprint, rootSentinel=matches[0],
                    homeSentinel=home.stdout.split()[0], generation=generation.stdout.strip(), boot=self.active)

    def prepare(self):
        self.connect()
        self.console.run('sh -c ' + shlex.quote('set -e; set -C; printf "%s\\n" ' + self.token + ' > ' + self.root_file + '; sync'))
        self.probe.require(self.probe.ssh('set -e; set -C; printf "%s\\n" ' + self.token + ' > "$HOME/' + self.home_file + '"; sync'))
        self.baseline = self.snapshot()

    def stop(self):
        if not self.active: return
        if self.console:
            self.console.logout()
            self.console = None
        result = self.host['command']([__import__('sys').executable,
                                      str(Path(self.host['__file__']).with_name('metal-run')), 'stop', self.active])
        if result.returncode: raise ValueError('cannot stop this category boot')
        self.wait(self.active, 'ended', timeout=180)
        self.active = None

    def attempt(self, broken):
        before = self.snapshot()
        self.stop()
        arguments = ['install-persistent', '--branch', self.args.branch, '--sha', self.args.sha]
        if broken: arguments += ['--inject', 'upgrade-no-ssh']
        boot = self.launch(arguments)
        result = json.loads(self.wait(boot, 'upgrade-result.json'))
        install = json.loads(self.read(boot, 'run.json'))
        if install.get('toolsCommit') != self.args.sha or self.read(boot, 'commit').strip() != self.args.sha:
            raise ValueError('install kernel and deployed tools differ from the category head')
        if result.get('previousGeneration') != before['generation']:
            raise ValueError('upgrade did not retain the original paired generation')
        for name in ('verification', 'rollback'):
            if not result.get(name): continue
            child = result[name]['boot']
            self.boots.append(child)
            run = json.loads(self.read(child, 'run.json'))
            if run.get('toolsCommit') != self.args.sha or run.get('upgradeParent') != boot:
                raise ValueError('verification used another tool commit or install')
            events = [json.loads(line) for line in self.read(child, 'events.jsonl').splitlines()]
            powers = [e['t'] for e in events if e.get('event') == 'power-on']
            if result[name].get('passed') and powers != [result[name].get('powerOn')]:
                raise ValueError('probe does not refer to the recorded power-on')
        self.active = self.launch(['boot', '--persistent'])
        self.connect()
        after = self.snapshot()
        expected = result['previousGeneration'] if broken else result['candidateGeneration']
        if after['generation'] != expected: raise ValueError('default slot did not select its paired userland')
        result['observations'] = [before, after]
        self.results['broken' if broken else 'good'] = result
        self.host['upgrade_cases']['upgrade'](result, broken, self.baseline)

    def generation_proofs(self):
        """Compile and execute production installer fault cases on the boot host's build container."""
        if 'generationProof' in self.results: return
        files = ('userspace/programs/src/persistent_install.rs', 'libs/libbreenix/src/layout.rs',
                 'libs/libbreenix/src/layout.tsv', 'kernel/src/fs/ext2/generation.rs',
                 'scripts/metal/efi-stub/src/slot_transaction.rs')
        base = self.env['BUILD_ROOT'] + '/upgrade-cases-' + self.anchor
        call = lambda *cmd, **kw: self.runner['in_build'](self.env, *cmd, capture_output=True, text=True, **kw)
        proof = {}
        owned = False
        try:
            made = call('mkdir', base)
            if made.returncode: raise ValueError('private generation test directory already exists')
            owned = True
            for name in files:
                value = call('git', '-C', self.env['BUILD_REPO'], 'show', self.args.sha + ':' + name)
                if value.returncode: raise ValueError('cannot read exact-head installer source')
                call('mkdir', '-p', str(Path(base + '/' + name).parent), check=True)
                call('tee', base + '/' + name, stdin=None, input=value.stdout, check=True)
            built = call('env', 'CARGO_BUILD_JOBS=4', 'rustc', '--edition=2021', '--test',
                         base + '/' + files[0], '-o', base + '/installer-tests')
            if built.returncode or re.search(r'(?m)^(warning|error)', built.stderr):
                raise ValueError('installer host harness did not compile cleanly: ' + built.stderr)
            for test, names in (
                ('interruption_enospc_and_io_error_at_each_operation_keep_current_bootable', ('enospc', 'io', 'interruption')),
                ('promotion_and_rollback_recover_paired_trees_byte_for_byte', ('paired-rollback',)),
                ('reclamation_retains_recovered_slots_and_shared_data_after_each_failure', ('reclamation',)),
                ('reclamation_preserves_directory_aliases_and_reclaims_plain_scratch', ('directory-aliases',))):
                namespace = ['unshare', '--mount', '--propagation', 'private', '--'] if names == ('directory-aliases',) else []
                result = call(*namespace, base + '/installer-tests', '--exact', 'tests::' + test, '--nocapture')
                receipt = dict(exit=result.returncode, executed='1 passed' in result.stdout,
                               stdout=result.stdout, stderr=result.stderr, commit=self.args.sha)
                for name in names: proof[name] = receipt
        finally:
            if owned: call('rm', '-rf', base)
        self.results['generationProof'] = proof

    def run_case(self, name):
        if name in ('good', 'broken'):
            self.attempt(name == 'broken')
        else:
            self.generation_proofs()
            self.host['upgrade_cases']['generations'](self.results['generationProof'], name == 'reclamation')
            if name == 'reclamation':
                old = self.results['good']['recoveredReferences']['PREVIOUS']
                broken = self.results['broken']
                if old in ('flat', 'empty') or old in broken['recoveredReferences'].values():
                    raise ValueError('physical reclamation needs an unreferenced completed generation')
                serial = self.read(broken['install'], 'serial.txt')
                if not re.search(r'(?m)^\[persistent\] reclaimed: ' + re.escape(old) + r'\r?$', serial):
                    raise ValueError('physical installer did not reclaim the unreferenced generation')
                self.results['physicalReclamation'] = dict(generation=old, install=broken['install'])


    def cleanup(self):
        try:
            if self.active and self.console:
                root = ('if [ -e ' + self.root_file + ' ]; then test "$(cat ' + self.root_file + ')" = '
                        + self.token + ' && rm ' + self.root_file + '; fi')
                self.console.run('sh -c ' + shlex.quote(root))
                home = ('f="$HOME/' + self.home_file + '"; if [ -e "$f" ]; then '
                        'test "$(cat "$f")" = ' + self.token + ' && rm "$f"; fi')
                self.probe.require(self.probe.ssh(home))
                self.console.run('sync')
        finally:
            try:
                self.stop()
            finally:
                self.scratch.cleanup()


def run(args, host):
    target = UpgradeSuite(args, host)
    boot = json.loads(target.read(args.boot, 'run.json'))
    manifest = json.loads(target.read(args.boot, 'definition/docs/suites/server.json'))
    effort = json.loads(target.read(args.boot, 'definition/docs/efforts/path.json'))
    fatal = json.loads(target.read(args.boot, 'definition/docs/boot-path.json'))['fatal'] + ['KERNEL PANIC:']
    records = dict(boot=args.boot, commit=args.sha, milestone='server/remote-upgrade', cases=[], cleanup=False, consoleGaps=[])
    captures = []
    try:
        target.prepare()
        records['baseline'] = target.baseline
        print('SUITE server START cases=4', flush=True)
        for name, _ in host['REMOTE_UPGRADE']:
            start = time.monotonic()
            try:
                target.run_case(name)
                result, reason = 'PASS', ''
            except Exception as error:
                result, reason = 'FAIL', type(error).__name__ + ': ' + str(error)
            ms = int((time.monotonic() - start)*1000)
            records['cases'].append(dict(id='remote-upgrade/'+name, result=result, ms=ms, reason=reason))
            print(f'SUITE server CASE remote-upgrade/{name} {result} ms={ms} msg={reason}', flush=True)
    finally:
        try:
            target.cleanup()
            records['cleanup'] = True
        except Exception as error:
            records['cleanupError'] = str(error)
        records.update(target.results)
        try:
            for boot_id in target.boots:
                run = json.loads(target.read(boot_id, 'run.json'))
                if run.get('toolsCommit') != args.sha or not run.get('evidenceEligible'):
                    raise ValueError('category boot is ineligible or used another tools commit')
                events = [json.loads(line) for line in target.read(boot_id, 'events.jsonl').splitlines()]
                records['consoleGaps'].extend(dict(event, boot=boot_id) for event in events if event.get('event') == 'console-gap')
                capture = target.read(boot_id, 'serial.txt')
                if any(re.search(pattern, capture) for pattern in fatal):
                    raise ValueError('fatal output in category boot ' + boot_id)
                captures.append(capture)
        except Exception as error:
            records.update(cleanup=False, auditError=str(error))
        target.remote('tee', target.root + args.boot + '/server-remote-upgrade.json', stdin=None,
                      input=json.dumps(records)+'\n')
    passed = sum(case['result'] == 'PASS' for case in records['cases'])
    print(f'SUITE server DONE passed={passed} failed={4-passed} skipped=0 total=4', flush=True)
    serial = '\n'.join(captures)
    verdict = host['score'](manifest, effort, records, serial, boot, args.sha, fatal)
    print(verdict, flush=True)
    return 0
