"""Drive ARM upgrade transactions with synthetic VM operations; never launch QEMU."""
import hashlib
import json
from pathlib import Path
import runpy
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
API = runpy.run_path(str(ROOT / 'scripts/metal/server-suite'))
CASES = runpy.run_path(str(ROOT / 'scripts/metal/upgrade_cases.py'))
SHA = 'a' * 40
PIN = 'SHA256:synthetic'
SITE = dict(CONSOLE_USER='root', CONSOLE_PASSWORD='fixture', OPERATOR_USER='fixture',
            OPERATOR_UID='1000', SERVER_SSH_HOST='127.0.0.1', SERVER_SSH_PORT='2222')


class Machine:
    def __init__(self, ops, boot, generation, fault=None):
        self.ops, self.boot, self.generation, self.fault = ops, boot, generation, fault
        self.run = ops.run
        self.directory = self.run / 'boots' / boot
        self.directory.mkdir(parents=True)
        (self.directory / 'started').write_text('1000')
        self.files = {'serial.txt': ('' if fault == 'upgrade-no-ssh' else
                      'Breenix ARM64 Kernel Starting\nBreenix login: '), 'events.jsonl': ''}

    def identity(self):
        return dict(id=self.boot, target='qemu', mode='persistent', bootedCommit=SHA,
                    toolsCommit=SHA, evidenceEligible=True, runId='run')

    def holds_lease(self): return self.ops.machine is self and not self.ops.stopped
    def ended(self, boot): return self.ops.stopped
    def reboot_site(self): return SITE
    def read(self, name, required=True): return self.files.get(name, '')
    def write(self, name, value): self.files[name] = json.dumps(value)
    def console(self, user, password):
        if user != 'root': raise ValueError('sentinels require root console')
        return Console(self)


class Console:
    def __init__(self, machine): self.machine = machine
    def service_log(self, name): return '[stdout] bsshd: host key ED25519 ' + PIN + ' (loaded)'
    def logout(self): pass
    def run(self, command):
        ops = self.machine.ops
        ops.calls.append(('console', command))
        if command.startswith('sha256sum '):
            return ops.sentinel + '  ' + command.split()[1] + '\n'
        if 'printf' in command:
            # The synthetic console retains exactly the driver's generated token.
            import re
            ops.token = re.search(r'remote-upgrade-[a-f0-9]+', command)[0]
            ops.sentinel = hashlib.sha256((ops.token + '\n').encode()).hexdigest()
        return ''


class Probe:
    def __init__(self, ops, directory):
        self.ops, self.known = ops, directory / 'known_hosts'
        self.common = ['StrictHostKeyChecking=yes', '-p', '2222']
        self.host, self.port = '127.0.0.1', 2222
        self.transcript = []
    def run_case(self, name, log): self.known.write_text('[127.0.0.1]:2222 ssh-ed25519 synthetic\n')
    def cleanup(self): pass
    def ssh(self, command, **kwargs):
        self.ops.calls.append(('ssh', command, kwargs.get('timeout')))
        failed = bool(self.ops.machine.fault)
        output = {'id -u': '1000', 'cat /usr/share/factory/generation': self.ops.machine.generation,
                  'cat /run/breenix/services': 'synthetic services'}.get(command, '')
        if command.startswith('sha256sum '): output = self.ops.sentinel + '  home\n'
        return SimpleNamespace(returncode=255 if failed else 0, stdout=output, stderr='')
    def require(self, result, expected=None):
        if result.returncode or (expected is not None and result.stdout.strip() != expected):
            raise ValueError('strict SSH failed')


class Ops:
    def __init__(self, run):
        self.run, self.calls, self.stopped = run, [], False
        self.slots = dict(current=None, previous=None, next=None)
        self.n = 0
        self.machine = Machine(self, 'initial', 'flat')
        self.fail_rollback = False
    def references(self):
        return {name.upper(): self.slots[name] or ('flat' if name == 'current' else 'empty')
                for name in self.slots}
    def stage(self, inject=None):
        self.calls.append(('stage', inject))
        self.n += 1
        self.candidate = dict(generation='vm-' + str(self.n), commit=SHA, injectedFault=inject)
        return self.candidate
    def install(self):
        self.calls.append(('install',))
        self.slots['next'] = self.candidate['generation']
        old = self.slots['previous']
        self.machine = Machine(self, 'install-' + str(self.n), 'installer')
        self.machine.files['serial.txt'] = '[persistent] install complete: files=1\n'
        # The good upgrade leaves the oldest seed unreferenced for the broken install.
        if old and old not in self.slots.values():
            self.machine.files['serial.txt'] += '[persistent] reclaimed: ' + old + '\n'
        if self.n >= 4:
            self.machine.files['serial.txt'] += '[persistent] reclaimed: vm-1\n'
        return self.machine
    def boot(self, slot):
        self.calls.append(('boot', slot))
        self.clock[0] = 1100
        fault = self.candidate['injectedFault'] if slot == 'next' else None
        if self.fail_rollback and slot == 'current' and self.n >= 4: fault = 'rollback-broken'
        self.machine = Machine(self, 'boot-' + str(len(self.calls)), self.slots[slot], fault)
        self.stopped = False
        return self.machine
    def stop(self, machine, console=None, force=False):
        self.calls.append(('stop', machine.boot, force))
        self.stopped = True
    def transition(self, operation):
        if not self.stopped: raise ValueError('slot transition while running')
        self.calls.append((operation,))
        if operation == 'promote':
            self.slots['previous'], self.slots['current'] = self.slots['current'], self.slots['next']
        self.slots['next'] = None


class DriverTest(unittest.TestCase):
    def setUp(self):
        self.module = runpy.run_path(str(ROOT / 'scripts/metal/server_upgrade_vm.py'))
        self.scratch = tempfile.TemporaryDirectory()
        self.addCleanup(self.scratch.cleanup)
        self.ops = Ops(Path(self.scratch.name))
        self.ops.clock = [1100]
        api = dict(API, Probe=lambda host, user, identity, directory, **kw: Probe(self.ops, directory),
                   parse_state=lambda _: ({}, {name: dict(starts=1, state='running', pid=10)
                                               for name in API['Services'].REQUIRED}))
        self.driver = self.module['QemuUpgrade'](SimpleNamespace(sha=SHA, boot='initial'), api,
                                                self.ops, identity=Path('/synthetic/key'))
        self.addCleanup(self.driver.scratch.cleanup)
        clock = patch.object(self.module['time'], 'time', side_effect=lambda: self.ops.clock[0])
        clock.start()
        self.addCleanup(clock.stop)
        sleep = patch.object(self.module['time'], 'sleep',
                             side_effect=lambda _: self.ops.clock.__setitem__(0, self.ops.clock[0] + 601))
        sleep.start()
        self.addCleanup(sleep.stop)

    def test_seed_good_and_both_faults_keep_sentinels_and_score_unchanged(self):
        self.driver.prepare()
        stages = [c for c in self.ops.calls if c[0] == 'stage']
        self.assertEqual(stages, [('stage', None), ('stage', None)])
        self.assertEqual(self.driver.baseline['generation'], 'vm-2')
        self.assertTrue(any(c[0] == 'console' and 'printf' in c[1] for c in self.ops.calls))
        self.assertTrue(any(c[0] == 'ssh' and 'printf' in c[1] for c in self.ops.calls))
        for name in ('good', 'broken', 'probe-failure'):
            self.driver.run_case(name)
        results = self.driver.results
        self.assertEqual([results[n]['outcome'] for n in ('good', 'broken', 'probe-failure')],
                         ['verified', 'rolled-back', 'rolled-back'])
        self.assertEqual(results['good']['recoveredReferences'], dict(CURRENT='vm-2', PREVIOUS='vm-1', NEXT='empty'))
        self.assertEqual(results['broken']['recoveredReferences'], dict(CURRENT='vm-3', PREVIOUS='vm-2', NEXT='empty'))
        self.assertIs(results['probe-failure']['candidateBooted'], True)
        self.assertIs(results['broken']['candidateBooted'], False)
        self.assertEqual(sum(c[0] == 'promote' for c in self.ops.calls), 3)
        self.assertEqual(sum(c[0] == 'rollback' for c in self.ops.calls), 2)
        self.assertTrue(any(c[0] == 'stop' and c[2] for c in self.ops.calls))
        for name, broken, bootable in (('good', False, False), ('broken', True, False), ('probe-failure', True, True)):
            result = results[name]
            CASES['upgrade'](result, broken, self.driver.baseline, bootable=bootable)
            self.assertEqual(result['verification']['powerOn'], 1000)
            self.assertGreater(result['verification']['sshAttempts'], 0)
        # Feed synthetic installer receipts to the unchanged remote-upgrade scorer.
        records = dict(boot='initial', commit=SHA, milestone='server/remote-upgrade', cleanup=True,
                       cases=[dict(id='remote-upgrade/' + name, result='PASS') for name, _ in API['REMOTE_UPGRADE']],
                       baseline=self.driver.baseline, **results)
        records['generationProof'] = {n: dict(exit=0, executed=True) for n in
                                     ('enospc', 'io', 'interruption', 'paired-rollback', 'reclamation', 'directory-aliases')}
        records['physicalReclamation'] = dict(generation='vm-1', install=results['broken']['install'])
        self.assertTrue(API['score'](json.loads((ROOT / 'docs/suites/server.json').read_text()),
            json.loads((ROOT / 'docs/efforts/path.json').read_text()), records,
            '[stdout] bsshd: host key ED25519 ' + PIN + ' (loaded)\n',
            self.ops.machine.identity() | dict(id='initial'), SHA, []).startswith('PASS:'))

    def test_expired_launch_never_gets_a_fresh_probe_deadline(self):
        probe = Probe(self.ops, Path(self.scratch.name))
        probe.run_case('host-key', '')
        with patch.object(self.module['time'], 'time', return_value=1601):
            receipt = self.module['probe_upgrade'](self.ops.machine, probe, SITE, self.driver.host,
                                                    'flat', PIN)
        self.assertFalse(receipt['passed'])
        self.assertEqual(receipt['powerOn'], 1000)
        self.assertFalse(any(c[0] == 'ssh' for c in self.ops.calls))

    def test_wrong_generation_or_missing_services_cannot_be_promoted(self):
        self.driver.prepare()
        self.ops.machine.generation = 'wrong'
        probe = self.driver.probe
        receipt = self.module['probe_upgrade'](self.ops.machine, probe, SITE, self.driver.host, 'vm-2', PIN)
        self.assertFalse(receipt['passed'])
        self.ops.machine.generation = 'vm-2'
        self.driver.host['parse_state'] = lambda _: ({}, {})
        receipt = self.module['probe_upgrade'](self.ops.machine, probe, SITE, self.driver.host, 'vm-2', PIN)
        self.assertFalse(receipt['passed'])
        self.assertEqual(sum(c[0] == 'promote' for c in self.ops.calls), 2)

    def test_failed_recovery_is_retained_and_never_claimed_as_rollback(self):
        self.driver.prepare()
        self.driver.run_case('good')
        self.ops.fail_rollback = True
        with self.assertRaises(ValueError): self.driver.run_case('broken')
        result = self.driver.results['broken']
        self.assertNotEqual(result.get('outcome'), 'rolled-back')
        self.assertFalse(result['rollback']['passed'])
        self.assertEqual(sum(c[0] == 'promote' for c in self.ops.calls), 3)


if __name__ == '__main__':
    unittest.main()
