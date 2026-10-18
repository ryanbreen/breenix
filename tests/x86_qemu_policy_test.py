"""Run the retired launchers: refusal must precede builds, slots and VM setup."""
import os
from pathlib import Path
import subprocess
import unittest

ROOT = Path(__file__).resolve().parents[1]
REFUSAL = ('x86 QEMU refused by the 2026-10-18 bare-metal-only policy (S23); '
           'use the R240 metal boot loop (scripts/metal/metal-run); '
           'operator override: BREENIX_ALLOW_X86_QEMU=1.')


class X86QemuPolicyTests(unittest.TestCase):
    def test_launchers_refuse_before_preparation(self):
        shell = [
            ('run.sh', '--x86'),
            ('run.sh', '--btrt', '--x86'),
            ('docker/qemu/run-blocking-io-oracle-gate.sh', '--arch', 'x86_64'),
            ('docker/qemu/run-ext2-lock-race-gate.sh', '--x86'),
            ('docker/qemu/run-fs-fault-gate.sh', '--x86'),
            ('scripts/ext2-acceptance.sh', 'x86', 'accept', '/unused'),
            ('scripts/test_tracing_via_gdb.sh', '--arch', 'x86_64'),
            ('tools/gdb-chat/scripts/gdb_session.sh', 'start'),
        ]
        shell += [('docker/qemu/' + name,) for name in (
            'run-blocking-recv-test.sh', 'run-boot-parallel.sh', 'run-dns-test.sh',
            'run-interactive.sh', 'run-keyboard-test.sh', 'run-kthread-parallel.sh',
            'run-kthread-test.sh', 'run-nonblock-eagain-test.sh', 'run-x86-boot-tests-only.sh',
            'run-x86-boot-tests.sh', 'run-x86-gate.sh', 'run-x86-prod-profile-boot-test.sh',
            'run-x86-smp-enum-gate.sh', 'run-x86-tty-oracle-gate.sh')]
        shell += [('scripts/' + name,) for name in (
            '772-diag-run-arm.sh', '772-dispatch-boot.sh', 'manual_test.sh', 'run-interactive-native.sh',
            'run_breenix.sh', 'run_exectest.sh', 'run_test.sh', 'test-workqueue.sh',
            'test_exectest.sh', 'test_kernel.sh', 'ci/ring3_check.sh')]
        commands = [(label[0], ['bash', str(ROOT / label[0]), *label[1:]]) for label in shell]
        commands += [(name, ['python3', str(ROOT / name), *args]) for name, args in (
            ('scripts/breenix_runner.py', []), ('scripts/gate-tree.py', []),
            ('scripts/ext2-acceptance.py', ['x86', 'accept', '/unused']),
            ('tools/gdb-chat/scripts/gdb_chat.py', ['--arch', 'x86_64']))]
        controller = ('import sys, tempfile; from pathlib import Path; '
                      'sys.path.insert(0, sys.argv[1]); from lib.gdb_controller import GDBSession; '
                      'GDBSession.SESSION_DIR = Path(tempfile.mkdtemp()); '
                      'GDBSession("policy-refusal", Path("/unused")).start()')
        commands += [('GDBSession.start', ['python3', '-c', controller,
                     str(ROOT / 'tools/gdb-chat/scripts')])]
        commands += [('scripts/test_timer_preemption.exp', ['expect', str(ROOT / 'scripts/test_timer_preemption.exp')])]
        binaries = Path(os.environ['BREENIX_POLICY_BIN_DIR'])
        commands += [(name, [str(binaries / name)]) for name in ('qemu-uefi', 'qemu-bios')]
        xtask = Path(os.environ['BREENIX_POLICY_XTASK'])
        commands += [('xtask ' + ' '.join(args), [str(xtask), *args]) for args in (
            ['boot-stages'], ['ring3-smoke'], ['ring3-enosys'], ['interactive'], ['interactive-test'],
            ['kthread-stress'], ['dns-test'], ['boot-test-btrt', '--arch', 'x86_64'],
            ['kthread-test', '--arch', 'x86_64'])]
        env = {k: v for k, v in os.environ.items() if not k.startswith('BREENIX_')}
        for name, command in commands:
            with self.subTest(launcher=name):
                result = subprocess.run(command, cwd=ROOT, env=env, capture_output=True, text=True, timeout=10)
                self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
                self.assertEqual(result.stdout, '', name)
                # The CLI adds its normal error prefix; the refusal remains a single line.
                self.assertEqual(result.stderr.removeprefix('error: ').strip(), REFUSAL, name)
                print('REFUSED ' + name + ': ' + result.stderr.strip(), flush=True)

    def test_cli_refuses_before_store_access(self):
        env = dict(os.environ)
        env.pop('BREENIX_ALLOW_X86_QEMU', None)
        env['BREENIX_RUNS_STORE'] = '/dev/null/store'
        result = subprocess.run([str(ROOT / 'tools/breenix-runs/.build/debug/breenix-runs'), 'run', 'x86'],
                                env=env, capture_output=True, text=True, timeout=10)
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertEqual(result.stdout, '')
        self.assertEqual(result.stderr, 'error: ' + REFUSAL + '\n')
        print('REFUSED breenix-runs run x86: ' + result.stderr.strip(), flush=True)

    def test_shell_requires_exact_override(self):
        for value in (None, '', '0', 'true', 'yes', '1'):
            with self.subTest(value=value):
                env = dict(os.environ)
                env.pop('BREENIX_ALLOW_X86_QEMU', None)
                if value is not None:
                    env['BREENIX_ALLOW_X86_QEMU'] = value
                result = subprocess.run(['bash', str(ROOT / 'scripts/x86-qemu-policy.sh')],
                                        env=env, capture_output=True, text=True, timeout=5)
                self.assertEqual(result.returncode, 0 if value == '1' else 1)
                self.assertEqual(result.stderr, '' if value == '1' else REFUSAL + '\n')


if __name__ == '__main__':
    unittest.main()
