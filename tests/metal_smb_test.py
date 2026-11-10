#!/usr/bin/env python3
"""Fixture checks for network-shares boot staging."""
import contextlib
import io
import importlib.machinery
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from types import SimpleNamespace
from unittest.mock import patch, Mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'scripts/metal'))
import metal_root


def load_script(name):
    loader = importlib.machinery.SourceFileLoader(name.replace('-', '_'), str(Path(metal_root.__file__).with_name(name)))
    spec = importlib.util.spec_from_loader(loader.name, loader)
    module = importlib.util.module_from_spec(spec)
    loader.exec_module(module)
    return module


class SmbStagingTest(unittest.TestCase):
    def test_only_network_shares_receives_private_fixture(self):
        mke2fs, debugfs = metal_root.require_e2fsprogs('mke2fs', 'debugfs')
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            tree = root / 'tree'
            (tree / 'etc/breenix').mkdir(parents=True)
            (tree / 'sbin').mkdir()
            for suite in ('network-shares', 'smoke'):
                (tree / 'sbin' / ('suite-' + suite)).write_bytes(b'\x7fELFfixture')
            image = root / 'root.img'
            subprocess.run([mke2fs, '-q', '-t', 'ext2', '-d', str(tree), '-F', str(image), '4096'], check=True, capture_output=True)
            fixture = {'SMB_TEST_USER': 'fixture-user', 'SMB_TEST_PASSWORD': 'fixture-secret-52',
                       'SMB_TEST_HOST': '192.0.2.42', 'SMB_TEST_PORT': '10445'}
            def read(command):
                return subprocess.run([debugfs, '-R', command, str(image)], check=True, capture_output=True)
            runner = load_script('metal-run')
            credential = root / 'smb-test.env'
            credential.write_text('SMB_TEST_USER=fixture-user\nSMB_TEST_PASSWORD="fixture-secret-52"\n')
            credential.chmod(0o600)
            private = root / 'service/smb-test.json'
            env = dict(fixture, SMB_TEST_INSTANCE='fixture-smb', METAL_INSTANCE='fixture-metal')
            env.pop('SMB_TEST_PASSWORD')
            env.pop('SMB_TEST_USER')
            commands = []
            def remote(_env, *args, **kwargs):
                commands.append(args)
                return subprocess.run([sys.executable, *args[1:]], **kwargs)
            output = io.StringIO()
            with contextlib.redirect_stdout(output), contextlib.redirect_stderr(output), \
                    patch.object(runner, 'SMB_CONFIG_PATH', str(private)), \
                    patch.object(runner, 'in_metal', side_effect=remote), \
                    patch.object(runner, 'incus', return_value=SimpleNamespace(returncode=0)) as server, \
                    patch.object(metal_root, 'SMB_CONFIG_PATH', private):
                runner.configure_smb(env, credential)
                server.assert_called_once()
                self.assertIn('/srv/smb/rw', server.call_args.args)
                self.assertIn('-delete', server.call_args.args)
                self.assertEqual(private.stat().st_mode & 0o777, 0o600)
                facts = metal_root.stage_root(image, 'suite', 'network-shares', {})
            cred = read('cat /etc/breenix/smb-test.cred').stdout.decode()
            self.assertEqual(cred, 'username=fixture-user\npassword=fixture-secret-52\n')
            stat = read('stat /etc/breenix/smb-test.cred').stdout.decode()
            self.assertIn('Mode:  0600', stat)
            self.assertRegex(stat, r'User:\s+0\s+Group:\s+0')
            self.assertEqual(read('cat /etc/breenix/smb-test-server').stdout.decode(), '192.0.2.42 10445\n')
            self.assertNotIn(fixture['SMB_TEST_PASSWORD'], output.getvalue() + json.dumps(facts) + repr(commands))
            metal_root.stage_root(image, 'suite', 'smoke', {}, smb_config=fixture)
            for name in ('smb-test.cred', 'smb-test-server'):
                self.assertIn(b'File not found', read('stat /etc/breenix/' + name).stderr)


    def test_boot_starts_helper_only_for_network_shares_and_deploy_packs_it(self):
        runner = load_script('metal-run')
        args = runner.argument_parser().parse_args(['boot', '--suite', 'network-shares', '--case', 'smb'])
        with tempfile.TemporaryDirectory() as tmp, \
                patch.object(runner, 'host_env', return_value={'METAL_INSTANCE': 'fixture'}), \
                patch.object(runner, 'resolve', return_value='a' * 40), \
                patch.object(runner, 'resolve_suites', side_effect=lambda env, sha, suite: suite), \
                patch.object(runner, 'in_metal', return_value=SimpleNamespace(returncode=0, stdout='claimed', stderr='')), \
                patch.object(runner, 'configure_smb') as configure, \
                patch.object(runner.subprocess, 'Popen') as processes, \
                patch.object(runner.Path, 'home', return_value=Path(tmp)):
            self.assertEqual(runner.boot(args), 0)
            configure.assert_called_once()
            self.assertEqual(sum('smb-restart' in call.args[0][1] for call in processes.call_args_list), 1)
            configure.reset_mock()
            processes.reset_mock()
            args.suite = 'smoke'
            self.assertEqual(runner.boot(args), 0)
            configure.assert_not_called()
            self.assertFalse(any('smb-restart' in call.args[0][1] for call in processes.call_args_list))
        deploy = Path(metal_root.__file__).with_name('metal-deploy').read_text()
        self.assertIn('users-hostssh smb-restart', deploy)

    def test_restart_records_are_answered_once_and_failures_leave_no_marker(self):
        helper = load_script('smb-restart')
        for fail in (False, True):
            with self.subTest(fail=fail), tempfile.TemporaryDirectory() as tmp:
                directory = Path(tmp)
                chunks = iter([b'SUITE network-shares SMBRESTART smb/server-restart seq=',
                               b'7\r\nSUITE network-shares SMBRESTART smb/server-restart seq=7\n'
                               b'SUITE network-shares SMBRESTART smb/other seq=8\n'
                               b'SUITE network-shares DONE passed=1 failed=0 skipped=0 total=1\n'])
                commands, transcripts = [], []
                def remote(*cmd, **kwargs):
                    if cmd[0] == 'tail':
                        return SimpleNamespace(returncode=0, stdout=next(chunks))
                    if cmd[0] == 'test':
                        return SimpleNamespace(returncode=1)
                    self.assertEqual(cmd[0], 'tee')
                    transcripts.append(json.loads(kwargs['input']))
                    return SimpleNamespace(returncode=0)
                def incus(*cmd, **kwargs):
                    commands.append(cmd)
                    return SimpleNamespace(returncode=1 if fail and 'systemctl' in cmd else 0)
                with patch.object(helper.time, 'sleep'), contextlib.redirect_stdout(io.StringIO()):
                    status = helper.watch('20261110T000000Z-metal', 'fixture-smb', remote, incus, minutes=1)
                self.assertEqual(status, int(fail))
                self.assertEqual(sum('systemctl' in cmd for cmd in commands), 1)
                self.assertEqual(commands[0], ('exec', 'fixture-smb', '--', 'systemctl', 'restart', 'smbd'))
                self.assertEqual(sum('/srv/smb/rw/.restarted-7' in cmd for cmd in commands), int(not fail))
                self.assertEqual(len(transcripts[-1]['requests']), 1)
                self.assertEqual(transcripts[-1]['requests'][0]['ok'], not fail)
                self.assertEqual(transcripts[-1]['requests'][0]['seq'], 7)


if __name__ == '__main__':
    unittest.main()
