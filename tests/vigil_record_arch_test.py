"""Local x86 identity and retained tracked changes, without a boot or real repository."""
import os
from pathlib import Path
import platform
import runpy
import subprocess
import tempfile
import unittest
from vigil_record_fixture import isolated_commands

ROOT = Path(__file__).resolve().parents[1]


@unittest.skipUnless(platform.system() == 'Darwin', 'recorder is macOS-only')
class VigilRecordArchTest(unittest.TestCase):
    def test_start_and_record_pass_x86_dirty_snapshot_and_processors(self):
        for verb in ('start', 'record'):
            with self.subTest(verb=verb), tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp)
                script = root / 'scripts/vigil-record.sh'
                script.parent.mkdir()
                script.write_bytes((ROOT / 'scripts/vigil-record.sh').read_bytes())
                script.chmod(0o755)
                cli = root / 'vigil-cli'
                cli.write_text('#!/bin/sh\nif [ "$#" = 1 ]; then echo "breenix start --arch --cpus"; exit 64; fi\nprintf "%s\\n" "$@" > "$FIXTURE_ARGS"\necho 20261018T120000Z-local\n')
                cli.chmod(0o755)
                env = {k: v for k, v in os.environ.items() if k not in ('VIGIL_BOOT_ID', 'VIGIL_TEST_HOME')}
                env.update(VIGIL_CLI=str(cli), BREENIX_VIGIL_ARCH='x86_64', BREENIX_VIGIL_CPUS='4',
                           FIXTURE_DIRTY=' M kernel/probe.rs\n M Cargo.toml\n', FIXTURE_ARGS=str(root / 'args'))
                isolated_commands(env, root)
                result = subprocess.run([str(script), verb, 'qemu', 'tests', '', str(root / 'serial'), '', '',
                                         '20261018T120000Z-local', 'a' * 40, '1', '2', '143'],
                                        env=env, capture_output=True, text=True, check=True)
                args = (root / 'args').read_text().splitlines()
                for option, expected in [('--arch', 'x86_64'), ('--dirty', '2'), ('--snapshot', 'c' * 40), ('--cpus', '4')]:
                    self.assertEqual(args[args.index(option) + 1], expected)
                self.assertIn('refs/vigil/boots/20261018T120000Z-local', (root / 'kept-ref').read_text())

    def test_local_x86_launcher_registers_before_guest_and_finishes_without_starting_one(self):
        module = runpy.run_path(str(ROOT / 'scripts/x86-gate-boot.py'))
        main = module['main']
        globals_ = main.__globals__
        calls = []
        class Answer:
            returncode = 0
            stdout = '20261018T120000Z-local'
        class Guest:
            pid = 123
            returncode = 1
            def poll(self): return 1
        def run(args, **kwargs):
            calls.append((args, kwargs.get('env')))
            return Answer()
        def popen(args, **kwargs):
            calls.append(('guest', None))
            return Guest()
        from unittest.mock import patch
        with tempfile.TemporaryDirectory() as tmp, \
             patch.object(globals_['subprocess'], 'run', run), \
             patch.object(globals_['subprocess'], 'Popen', popen), \
             patch.dict(globals_, stop=lambda guest: None):
            # Relay API is replaced: the fixture has no sockets and no child process.
            with patch.dict(globals_['relay_api'], live_serial_command=lambda args, logs: (args, []), close_relays=lambda relays: None), \
                 patch.object(globals_['sys'], 'argv', ['launcher', tmp + '/kernel', tmp + '/user', 'qemu-system-x86_64', '-smp', '4']):
                self.assertEqual(main(), 1)
        self.assertEqual(calls[0][0][1:4], ['start', 'qemu', 'tests'])
        self.assertEqual(calls[0][1]['BREENIX_VIGIL_ARCH'], 'x86_64')
        self.assertEqual(calls[0][1]['BREENIX_VIGIL_CPUS'], '4')
        self.assertEqual(calls[1][0], 'guest')
        self.assertEqual(calls[-1][0][1:], ['finish', '20261018T120000Z-local', '1'])


if __name__ == '__main__':
    unittest.main()
