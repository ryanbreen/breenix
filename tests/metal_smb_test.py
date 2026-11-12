#!/usr/bin/env python3
"""Fixture checks for SMB suite boot staging."""
import contextlib
import io
import importlib.machinery
import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from types import SimpleNamespace
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'scripts/metal'))
import metal_root


def load_script(name):
    loader = importlib.machinery.SourceFileLoader(name.replace('-', '_'), str(Path(metal_root.__file__).with_name(name)))
    spec = importlib.util.spec_from_loader(loader.name, loader)
    module = importlib.util.module_from_spec(spec)
    loader.exec_module(module)
    return module


class SmbStagingTest(unittest.TestCase):
    @contextlib.contextmanager
    def gui_root(self):
        mke2fs, debugfs = metal_root.require_e2fsprogs('mke2fs', 'debugfs')
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            tree = root / 'tree'
            for name in ('etc/breenix', 'sbin', 'bin', 'mnt'):
                (tree / name).mkdir(parents=True)
            for name in ('sbin/suite-gui-apps', 'sbin/suite-console', 'sbin/suite-smoke',
                         'sbin/init', 'bin/getty', 'bin/login'):
                (tree / name).write_bytes(b'\x7fELFfixture')
            image = root / 'root.img'
            subprocess.run([mke2fs, '-q', '-t', 'ext2', '-d', str(tree), '-F', str(image), '4096'],
                           check=True, capture_output=True)
            def read(command):
                return subprocess.run([debugfs, '-R', command, str(image)], check=True, capture_output=True)
            config = {'SMB_TEST_USER': 'fixture-user', 'SMB_TEST_PASSWORD': 'fixture-secret-52',
                      'SMB_TEST_HOST': '192.0.2.42', 'SMB_TEST_PORT': '10445'}
            yield image, read, config

    def test_gui_apps_guskit_stages_boot_mounts(self):
        for case in (None, 'guskit', 'guskit/smb-save-open'):
            with self.subTest(case=case), self.gui_root() as (image, read, config):
                output = io.StringIO()
                with contextlib.redirect_stdout(output), contextlib.redirect_stderr(output):
                    facts = metal_root.stage_root(image, 'suite', 'gui-apps', {}, smb_config=config,
                                                  **({'case': case} if case else {}))
                expected = {
                    '/etc/breenix/smb-test.cred': ('username=fixture-user\npassword=fixture-secret-52\n', '0600'),
                    '/etc/breenix/smb-test-server': ('192.0.2.42 10445\n', '0600'),
                    '/etc/shares': ('//192.0.2.42/rw /mnt/guskit-rw smb credfile=/etc/breenix/smb-test.cred,port=10445\n'
                                    '//192.0.2.42/ro /mnt/guskit-ro smb credfile=/etc/breenix/smb-test.cred,port=10445\n', '0644')}
                for path, (content, mode) in expected.items():
                    self.assertEqual(read('cat ' + path).stdout.decode(), content)
                    stat = read('stat ' + path).stdout.decode()
                    self.assertIn('Mode:  ' + mode, stat)
                    self.assertRegex(stat, r'User:\s+0\s+Group:\s+0')
                for path in ('/mnt/guskit-rw', '/mnt/guskit-ro'):
                    stat = read('stat ' + path).stdout.decode()
                    self.assertIn('Type: directory', stat)
                    self.assertIn('Mode:  0755', stat)
                    self.assertRegex(stat, r'User:\s+0\s+Group:\s+0')
                self.assertNotIn(config['SMB_TEST_PASSWORD'], output.getvalue() + json.dumps(facts))

    def test_sequences_without_gui_apps_do_not_stage_guskit_mounts(self):
        for suite in ('console,smoke',):
            with self.subTest(suite=suite), self.gui_root() as (image, read, config):
                selected = metal_root.smb_gui_selection('suite', suite)
                # Inspect the staged image even if the predicate is wrong.
                facts = metal_root.stage_root(image, 'suite', suite, {}, smb_config=config)
                self.assertFalse(selected)
                self.assertEqual(facts['suite'], suite)
                self.assertEqual(read('cat /etc/breenix/boot-target').stdout.decode(),
                                 'suite ' + suite.split(',')[0] + ' hostkeys host-channel\n')
                for path in ('/etc/shares', '/etc/breenix/smb-test.cred',
                             '/mnt/guskit-rw', '/mnt/guskit-ro'):
                    self.assertIn(b'File not found', read('stat ' + path).stderr)

    def test_gui_apps_sequence_configures_smb_before_boot(self):
        runner = load_script('metal-run')
        for suite in ('gui-apps,console', 'console,gui-apps', 'console,smoke'):
            for case in (None, 'guskit') if 'gui-apps' in suite else (None,):
                with self.subTest(suite=suite, case=case), tempfile.TemporaryDirectory() as tmp, \
                        patch.object(runner, 'host_env', return_value={}), \
                        patch.object(runner, 'resolve', return_value='a' * 40), \
                        patch.object(runner, 'resolve_suites', side_effect=lambda env, sha, suites: suites), \
                        patch.object(runner, 'configure_smb') as configure, \
                        patch.object(runner, 'in_metal', return_value=SimpleNamespace(returncode=0, stdout='claimed', stderr='')) as remote, \
                        patch.object(runner.subprocess, 'Popen') as processes, \
                        patch.object(runner.Path, 'home', return_value=Path(tmp)):
                    args = runner.argument_parser().parse_args(
                        ['boot', '--suite', suite] + (['--case', case] if case else []))
                    if case:
                        with self.assertRaisesRegex(SystemExit, '--case needs one production suite'):
                            runner.boot(args)
                        remote.assert_not_called()
                        configure.assert_not_called()
                        continue
                    self.assertEqual(runner.boot(args), 0)
                    self.assertTrue(remote.called)
                    self.assertTrue(processes.called)
                    if 'gui-apps' in suite:
                        configure.assert_called_once()
                    else:
                        configure.assert_not_called()

    def test_console_and_other_gui_selection_remove_smb_mounts(self):
        for mode, suite, case in (('console', None, None), ('suite', 'gui-apps', 'bterm')):
            with self.subTest(mode=mode, case=case), self.gui_root() as (image, read, config):
                metal_root.stage_root(image, 'suite', 'gui-apps', {}, smb_config=config)
                self.assertIn(b'Inode:', read('stat /etc/shares').stdout)
                site = {'CONSOLE_USER': 'root', 'CONSOLE_PASSWORD': 'fixture_password_only_12345',
                        'CONSOLE_IDENTITY_KEY': 'ab' * 32}
                with patch.object(metal_root, 'console_hash', return_value='$6$fixture$hash'):
                    metal_root.stage_root(image, mode, suite, site, smb_config=config,
                                          **({'case': case} if case else {}))
                for path in ('/etc/breenix/smb-test.cred', '/etc/breenix/smb-test-server',
                             '/etc/shares', '/mnt/guskit-rw', '/mnt/guskit-ro'):
                    self.assertIn(b'File not found', read('stat ' + path).stderr)

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
                helper = load_script('smb-restart')
                def claimed(*args, **kwargs):
                    return SimpleNamespace(returncode=1 if args[0] == 'test' else 0, stdout='{"status":"claimed"}')
                self.assertTrue(helper.prepare('20261110T000000Z-metal', claimed, runner.incus, 'fixture-smb'))
                self.assertEqual(server.call_count, 2)
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
            for case, needs_smb in ((None, True), ('guskit', True), ('guskit/smb-save-open', True), ('bterm', False)):
                with self.subTest(case=case):
                    configure.reset_mock()
                    processes.reset_mock()
                    args = runner.argument_parser().parse_args(
                        ['boot', '--suite', 'gui-apps'] + (['--case', case] if case else []))
                    self.assertEqual(runner.boot(args), 0)
                    self.assertEqual(configure.call_count, int(needs_smb))
                    self.assertEqual(sum('smb-restart' in call.args[0][1] for call in processes.call_args_list),
                                     int(needs_smb))
                    build = next(call.args[0] for call in processes.call_args_list if '_build' in call.args[0])
                    self.assertEqual('--smb-helper' in build, needs_smb)
        deploy = Path(metal_root.__file__).with_name('metal-deploy').read_text()
        self.assertIn('users-hostssh smb-restart', deploy)

    def test_restart_records_are_answered_once_and_failures_leave_no_marker(self):
        helper = load_script('smb-restart')
        for fail in (False, True):
            with self.subTest(fail=fail), tempfile.TemporaryDirectory() as tmp:
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

    def test_deployed_metal_run_starts(self):
        import shlex
        import shutil
        deploy = Path(metal_root.__file__).with_name("metal-deploy").read_text()
        line = next(line for line in deploy.splitlines() if line.startswith("pack metal-run "))
        names = shlex.split(line.rstrip().rstrip(chr(92)))
        with tempfile.TemporaryDirectory() as tmp:
            for name in names[1:]:
                source = Path(metal_root.__file__).parent / name
                if source.is_file():
                    shutil.copy(source, tmp)
            result = subprocess.run([sys.executable, str(Path(tmp) / "metal-run"), "--help"],
                                    capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr)

    def test_gui_stop_start_and_cleanup(self):
        helper = load_script('smb-restart')
        for ending in ('done', 'ended'):
            with self.subTest(ending=ending):
                chunks = iter([b'SUITE gui-apps SMBSTOP guskit/smb-share-gone seq=1\n'
                               b'SUITE gui-apps SMBSTOP guskit/smb-share-gone seq=1\n'
                               b'SUITE gui-apps SMBSTART guskit/smb-share-gone seq=2\n'
                               b'SUITE gui-apps SMBSTOP guskit/smb-share-gone seq=3\n' +
                               (b'SUITE gui-apps DONE passed=1 failed=0 skipped=0 total=1\n'
                                if ending == 'done' else b'')])
                commands, transcripts = [], []
                def remote(*cmd, **kwargs):
                    if cmd[0] == 'tail':
                        return SimpleNamespace(returncode=0, stdout=next(chunks))
                    if cmd[0] == 'test':
                        return SimpleNamespace(returncode=int(ending != 'ended'))
                    self.assertEqual(cmd[0], 'tee')
                    transcripts.append(json.loads(kwargs['input']))
                    return SimpleNamespace(returncode=0)
                def incus(*cmd, **kwargs):
                    commands.append(cmd)
                    return SimpleNamespace(returncode=0)
                with patch.object(helper.time, 'sleep'), contextlib.redirect_stdout(io.StringIO()):
                    status = helper.watch('20261110T000000Z-metal', 'fixture-smb', remote, incus, minutes=1)
                self.assertEqual(status, 0)
                self.assertEqual([cmd[-2:] for cmd in commands],
                                 [('stop', 'smbd'), ('start', 'smbd'), ('stop', 'smbd'), ('start', 'smbd')])
                self.assertEqual([(r['seq'], r['ok']) for r in transcripts[-1]['requests']],
                                 [(1, True), (2, True), (3, True)])


if __name__ == '__main__':
    unittest.main()
