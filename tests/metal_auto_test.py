#!/usr/bin/env python3
"""Caller-side build routing with synthetic SSH/build boundaries; no hardware."""
import contextlib
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import unittest
from types import SimpleNamespace
from unittest.mock import patch

from metal_tools_test import load_script, StateTest, boot, m

auto = load_script('metal-boot-auto')


class LoadChoiceTest(unittest.TestCase):
    def test_below_and_over_cpuset_limit(self):
        for load, expected in ((31.9, 'beast'), (32, 'mac'), (78.2, 'mac')):
            with self.subTest(load=load):
                decision, line = auto.choose(dict(load5=load, cpuCount=32), None)
                self.assertEqual(decision['builder'], expected)
                self.assertEqual((decision['load5'], decision['limit']), (load, 32))
                self.assertEqual(len(line.splitlines()), 1)
        self.assertEqual(line, "beast's 5-minute load average is 78.2, over the build limit of 32 "
                         "(the build container's CPUs), so this build runs on this Mac.")

    def test_override_and_both_forcing_flags(self):
        for override, force, expected in (('80', None, 'beast'), ('20', None, 'mac'),
                                           (None, 'mac', 'mac'), (None, 'beast', 'beast')):
            with self.subTest(override=override, force=force):
                decision, line = auto.choose(dict(load5=40, cpuCount=32), None, override, force)
                self.assertEqual(decision['builder'], expected)
                if override:
                    self.assertEqual(decision['limit'], float(override))
                    self.assertIn('BREENIX_METAL_BUILD_LOAD_LIMIT', line)
                if force:
                    self.assertEqual(decision['reason'], 'forced')
                    self.assertIn('--build-', line)
        for invalid in ('0', '-1', 'nan', 'inf', 'garbage', ''):
            with self.subTest(invalid=invalid), self.assertRaisesRegex(ValueError, 'positive finite'):
                auto.choose(None, 'unreadable', invalid)

    def test_unreadable_falls_back_and_forced_beast_is_honoured(self):
        for force, expected in ((None, 'mac'), ('beast', 'beast')):
            decision, line = auto.choose(None, 'SSH load probe timed out', force=force)
            self.assertEqual(decision['builder'], expected)
            self.assertIsNone(decision['load5'])
            self.assertIn('SSH load probe timed out', line)

    def test_ssh_probe_uses_affinity_and_rejects_unreadable_readings(self):
        valid = SimpleNamespace(returncode=0, stdout=json.dumps(dict(load5=78.2, cpus=[0, 2, 4])))
        with patch.object(auto.subprocess, 'run', return_value=valid) as ssh:
            self.assertEqual(auto.probe('fixture'), (dict(load5=78.2, cpuCount=3), None))
            self.assertIn('sched_getaffinity(0)', ssh.call_args.args[0][-1])
            self.assertIn('getloadavg()[1]', ssh.call_args.args[0][-1])
            self.assertEqual(ssh.call_args.kwargs['timeout'], 15)
        for result in (SimpleNamespace(returncode=255, stdout=''),
                       SimpleNamespace(returncode=0, stdout='bad'),
                       SimpleNamespace(returncode=0, stdout='{"load5":1,"cpus":[]}')):
            with patch.object(auto.subprocess, 'run', return_value=result):
                reading, error = auto.probe('fixture')
                self.assertIsNone(reading)
                self.assertTrue(error)

    def test_cli_routes_forced_sides_and_records_choice(self):
        for flag, load, local in ((None, 10, False), (None, 78.2, True),
                                  ('--build-here', 10, True), ('--build-on-beast', 78.2, False)):
            with self.subTest(flag=flag):
                args = ['metal-boot-auto', '--mode', 'tests', '--branch', 'metal-fixture/test']
                if flag:
                    args.append(flag)
                process = SimpleNamespace(stdout=io.StringIO('{"id":"20261103T000000Z-metal"}\n'),
                                          returncode=0, wait=lambda: 0)
                with patch.object(sys, 'argv', args), patch.dict(os.environ, {}, clear=True), \
                     patch.object(auto, 'probe', return_value=(dict(load5=load, cpuCount=32), None)), \
                     patch.object(auto.subprocess, 'check_output', side_effect=lambda command, **kw:
                                  'b' * 40 if 'rev-parse' in command else 'a' * 40 + '\tref\n'), \
                     patch.object(auto.subprocess, 'run', return_value=SimpleNamespace(
                                  returncode=0, stdout='b' * 40 + '\n')) as run, \
                     patch.object(auto.subprocess, 'Popen') as build, contextlib.redirect_stdout(io.StringIO()):
                    build.return_value.__enter__.return_value = process
                    self.assertEqual(auto.main(), 0)
                remote = run.call_args.args[0]
                self.assertEqual(remote[:4], ['ssh', '-o', 'BatchMode=yes', 'beast'])
                import shlex
                forwarded = shlex.split(remote[-1])
                choice = json.loads(forwarded[forwarded.index('--build-choice') + 1])
                self.assertEqual(choice['builder'], 'mac' if local else 'beast')
                self.assertEqual(choice['load5'], load)
                self.assertEqual(build.call_count, int(local))
                if local:
                    self.assertIn('--prebuilt', forwarded)
                    self.assertIn('testing', build.call_args.args[0])
                    command = build.call_args.args[0]
                    self.assertEqual(command[command.index('--tools-commit') + 1], 'b' * 40)
                    self.assertEqual(forwarded[forwarded.index('--sha') + 1], 'a' * 40)
                else:
                    self.assertNotIn('--prebuilt', forwarded)

    def test_mac_refuses_tools_mismatch_before_build_or_upload(self):
        process = SimpleNamespace(stdout=io.StringIO('{"id":"20261103T000000Z-metal"}\n'),
                                  returncode=0, wait=lambda: 0)
        with patch.object(sys, 'argv', ['metal-boot-auto', '--mode', 'tests']), \
             patch.dict(os.environ, {}, clear=True), \
             patch.object(auto, 'probe', return_value=(dict(load5=78.2, cpuCount=32), None)), \
             patch.object(auto.subprocess, 'check_output', return_value='a' * 40), \
             patch.object(auto.subprocess, 'run', return_value=SimpleNamespace(
                          returncode=0, stdout='b' * 40 + '\n')), \
             patch.object(auto.subprocess, 'Popen') as build, contextlib.redirect_stdout(io.StringIO()):
            build.return_value.__enter__.return_value = process
            with self.assertRaisesRegex(ValueError, '--tools-commit ' + 'b' * 40):
                auto.main()
            build.assert_not_called()

    def test_tools_preflight_is_bounded_and_refuses_unreadable_identity(self):
        for result in (SimpleNamespace(stdout='dirty-or-missing'),
                       subprocess.TimeoutExpired('ssh', 15),
                       subprocess.CalledProcessError(255, 'ssh'), OSError('unavailable')):
            with self.subTest(result=result), \
                 patch.object(auto.subprocess, 'check_output', return_value='a' * 40), \
                 patch.object(auto.subprocess, 'run') as ssh:
                if isinstance(result, Exception):
                    ssh.side_effect = result
                else:
                    ssh.return_value = result
                with self.assertRaisesRegex(ValueError, 'refusing before Mac build'):
                    auto.preflight_tools(Path('/fixture'), 'fixture', 'HEAD')
                self.assertEqual(ssh.call_args.kwargs['timeout'], 15)
                self.assertTrue(ssh.call_args.kwargs['check'])


class ChoiceRecordTest(StateTest):
    def test_host_choice_reaches_launch_and_record(self):
        metal = load_script('metal-run')
        decision, _ = auto.choose(dict(load5=2, cpuCount=4), None)
        with patch.object(sys, 'argv', ['metal-run', 'boot', '--mode', 'tests', '--build-choice', json.dumps(decision)]), \
             patch.object(metal, 'host_env', return_value={}), \
             patch.object(metal, 'resolve', return_value='a' * 40), \
             patch.object(metal, 'in_metal', return_value=SimpleNamespace(returncode=0, stdout='claimed', stderr='')) as launch, \
             patch.object(metal.Path, 'home', return_value=m.STATE), patch.object(metal.subprocess, 'Popen') as build:
            self.assertEqual(metal.main(), 0)
        command = launch.call_args.args
        self.assertEqual(json.loads(command[command.index('--build-choice') + 1]), decision)
        self.assertIn('_build', build.call_args.args[0])
        args = SimpleNamespace(id='20261103T000000Z-metal', mode='tests', payload='image', target='main',
                               task='', commit='a' * 40, inject=None, suite=None, build_choice=decision)
        with patch.object(m, 'load_site', return_value={}):
            driver = boot.Driver(args)
            driver.create_record()
        self.assertEqual(m.read_json(driver.dir / 'metal-run.json')['buildChoice'], decision)
        self.assertEqual((driver.dir / 'run.json').read_bytes(), (driver.dir / 'metal-run.json').read_bytes())


class RetentionTest(unittest.TestCase):
    def test_upload_removes_partial_on_failure_and_prunes_stale_payloads(self):
        local = load_script('metal-build-local')
        with tempfile.TemporaryDirectory() as temporary:
            home = Path(temporary)
            base = home / '.local/state/breenix-metal/prebuilt'
            base.mkdir(parents=True)
            for name in ('20261001T000000Z-metal', '20261002T000000Z-metal.upload'):
                path = base / name
                path.mkdir()
                os.utime(path, (0, 0))
            recent = base / '20261103T000000Z-metal'
            recent.mkdir()
            # Execute the actual upload receiver against a broken stream.
            result = subprocess.run([sys.executable, '-c', local.UPLOAD, '20261103T000001Z-metal'],
                                    input=b'broken tar', capture_output=True, env=dict(os.environ, HOME=str(home)))
            self.assertNotEqual(result.returncode, 0)
            self.assertEqual(sorted(p.name for p in base.iterdir()), ['.upload.lock', recent.name])
            import tarfile
            stream = io.BytesIO()
            with tarfile.open(fileobj=stream, mode='w') as archive:
                entry = tarfile.TarInfo('payload')
                entry.size = 7
                archive.addfile(entry, io.BytesIO(b'fixture'))
            subprocess.run([sys.executable, '-c', local.UPLOAD, '20261103T000002Z-metal'],
                           input=stream.getvalue(), capture_output=True, check=True,
                           env=dict(os.environ, HOME=str(home)))
            self.assertEqual((base / '20261103T000002Z-metal/payload').read_bytes(), b'fixture')
            self.assertFalse(list(base.glob('*.upload')))

    def test_local_outputs_are_bounded_and_abandoned_trees_removed(self):
        local = load_script('metal-build-local')
        with tempfile.TemporaryDirectory() as temporary:
            cache = Path(temporary)
            for index in range(5):
                path = cache / f'breenix-metal-20261103t00000{index}z-metal-out'
                path.mkdir()
                os.utime(path, (time.time() - index, time.time() - index))
            abandoned = cache / 'breenix-metal-20261103t000000z-metal-tmp'
            abandoned.mkdir()
            local.prune_outputs(cache)
            self.assertFalse(abandoned.exists())
            self.assertEqual(len(list(cache.glob('*-out'))), 3)


if __name__ == '__main__':
    unittest.main()
