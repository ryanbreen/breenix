"""Console-session lifecycle rules, with synthetic input and management state."""
from pathlib import Path
import os
import subprocess
import sys
import tempfile
import unittest
from types import SimpleNamespace
from unittest.mock import Mock, patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
import metal_tools_test as existing

boot, m, machine = existing.boot, existing.m, existing.machine


class SessionTest(existing.StateTest):
    def setUp(self):
        super().setUp()
        self.args = SimpleNamespace(id='20261017T000009Z-metal', mode='console', payload='image',
                                    session=True, idle_minutes=2, input=None, disk=None,
                                    target='branch:fixture', task='', commit='a' * 40, inject=None, nmi=False)
        with patch.object(m, 'load_site', return_value={}):
            self.driver = boot.Driver(self.args)
        self.driver.event, self.driver.phase = Mock(), Mock()
        self.driver.dir.mkdir(parents=True)
        self.driver.t0 = 100

    def test_four_hour_ceiling_is_written_from_observed_power_on_and_expires_at_boundary(self):
        d = self.driver
        with patch.object(m, 'rf', return_value={'startedAt': 102}), \
             patch.object(m, 'rf_get', return_value={'Boot': {'BootSourceOverrideEnabled': 'Once',
                                                            'BootSourceOverrideTarget': 'UefiHttp'}}), \
             patch.object(m, 'update_lease') as update, patch.object(m, 'now', return_value=100):
            d.power_on()
        self.assertEqual(d.lease_ceiling, 102 + 4 * 3600)
        self.assertEqual(update.call_args.kwargs['ceilingAt'], d.lease_ceiling)
        d.kernel_started = d.complete_at = d.request_at = 110
        with patch.object(m, 'now', return_value=d.lease_ceiling - 1):
            d.check_deadlines()  # Neither output silence nor the usual console deadline applies.
        with patch.object(m, 'now', return_value=d.lease_ceiling):
            with self.assertRaises(boot.Finish) as result:
                d.check_deadlines()
        self.assertEqual(result.exception.status, m.EXIT_DONE)
        self.assertFalse(result.exception.hung)

    def test_only_exact_kernel_idle_line_ends_a_session_and_driver_never_types(self):
        d = self.driver
        d.kernel_started = 110
        d.final_marker = ('ordinary completion', 'line')
        d.final_seen = True
        d.console_done = True
        d.console_send = Mock()
        for text in ['Breenix login: ', 'Password: ', 'ordinary completion',
                     '[console] input idle 30 min', 'echo [console] input idle 2 min']:
            d.on_line(text)
        d.console_send.assert_not_called()
        with self.assertRaises(boot.Finish) as result:
            d.on_line('[console] input idle 2 min')
        self.assertEqual(result.exception.status, m.EXIT_DONE)
        self.assertIn('input idle', result.exception.reason)

    def test_stop_interrupts_session_and_uses_immediate_power_off_without_nmi(self):
        d = self.driver
        (d.dir / 'stop-request').touch()
        with self.assertRaises(boot.Finish) as result:
            d.checkpoint()
        self.assertEqual(result.exception.status, m.EXIT_STOPPED)
        with patch.object(m, 'force_off_and_wait', return_value=True) as off, patch.object(m, 'rf') as rf:
            self.assertTrue(d.power_off(result.exception.hung))
        off.assert_called_once()
        rf.assert_not_called()
        d.t0 = None
        with self.assertRaises(boot.Finish) as result:
            d.checkpoint()
        self.assertEqual(result.exception.status, m.EXIT_CANCELLED)

    def test_reaper_keeps_session_past_normal_ceiling_then_powers_off_at_session_ceiling(self):
        d = self.driver
        ceiling = 100 + boot.ceiling_seconds(self.args)
        lease = {'id': d.id, 'kind': 'console-session', 'state': 'powered',
                 'heartbeat': 2000, 'ceilingAt': ceiling}
        m.write_json(m.STATE / m.LEASE, lease)
        reaper = machine.Machine({})
        with patch.object(m, 'now', return_value=2000), patch.object(m, 'holder_alive', return_value=True), \
             patch.object(m, 'force_off_and_wait') as off:
            reaper.reap()
        off.assert_not_called()
        lease['heartbeat'] = ceiling
        m.write_json(m.STATE / m.LEASE, lease)
        with patch.object(m, 'now', return_value=ceiling), patch.object(m, 'holder_alive', return_value=True), \
             patch.object(m, 'force_off_and_wait', return_value=True) as off, \
             patch.object(machine, 'end_session_side_effects'):
            reaper.reap()
        off.assert_called_once()
        self.assertEqual((d.dir / 'exit-status').read_text(), '0\n')
        self.assertIsNone(m.read_lease())

    def test_operator_record_cannot_be_scored_even_without_a_definition(self):
        d = self.driver
        d.dir.rmdir()
        d.create_record()
        record = m.read_json(d.dir / 'run.json')
        self.assertEqual(record['kind'], 'console-session')
        self.assertFalse(record['evidenceEligible'])
        with self.assertRaisesRegex(ValueError, 'never scored'):
            boot.metal_score.score(d.dir)
        d.load_definition()
        self.assertIsNone(d.final_marker)
        self.assertEqual(d.required_stages, [])

    def test_session_options_reject_automated_typing_install_and_bad_idle_before_host_access(self):
        runner = existing.load_script('metal-run')
        for changes in [dict(mode='shell'), dict(input='sol'), dict(disk='install'),
                        dict(idle_minutes=0), dict(idle_minutes=241), dict(disk='root')]:
            args = SimpleNamespace(**(vars(self.args) | changes))
            with self.assertRaises(SystemExit), patch.object(runner, 'host_env') as host:
                runner.boot(args)
            host.assert_not_called()

    def test_host_runner_forwards_session_and_idle_without_automated_input(self):
        runner = existing.load_script('metal-run')
        args = SimpleNamespace(**(vars(self.args) | dict(sha=None, branch='fixture', suite=None,
                               no_wait=False, serve_as=None, content_type=None, ramdisk=True)))
        env = {'METAL_INSTANCE': 'fixture'}
        with patch.object(runner, 'host_env', return_value=env), \
             patch.object(runner, 'resolve', return_value='a' * 40), \
             patch.object(runner, 'in_metal', return_value=SimpleNamespace(returncode=0, stdout='claimed', stderr='')) as send, \
             patch.object(runner.subprocess, 'Popen'), patch.object(runner.Path, 'home', return_value=m.STATE):
            self.assertEqual(runner.boot(args), 0)
        command = send.call_args.args
        self.assertIn('--session', command)
        self.assertEqual(command[command.index('--idle-minutes') + 1], '2')
        self.assertNotIn('--input', command)

    def test_session_shutdown_skips_scoring(self):
        d = self.driver
        definition = d.dir / 'definition/docs/metal/boot-path.json'
        definition.parent.mkdir(parents=True)
        definition.write_text('{}')
        # Drive the real Finish handler, including power-off, with no hardware calls.
        d.create_record = Mock()
        d.claim = d.prepare_payload = d.preflight = d.do_stage = d.start_console = d.power_on = Mock()
        d.watch = Mock(side_effect=boot.Finish(m.EXIT_DONE, 'console input idle for 2 minutes'))
        d.power_off = Mock(return_value=True)
        d.cleanup = Mock()
        with patch.object(boot.metal_score, 'score') as score:
            self.assertEqual(d.run(), m.EXIT_DONE)
        d.power_off.assert_called_once_with(False)
        score.assert_not_called()
        self.assertFalse((d.dir / 'milestone-verdict.json').exists())


class TargetWriterTest(unittest.TestCase):
    def test_two_minute_console_target_survives_image_readback(self):
        import shutil
        prefix = Path('/opt/homebrew/opt/e2fsprogs/sbin')
        debugfs = shutil.which('debugfs') or str(prefix / 'debugfs')
        mke2fs = shutil.which('mke2fs') or str(prefix / 'mke2fs')
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            tree = root / 'tree'
            (tree / 'etc/breenix').mkdir(parents=True)
            (tree / 'sbin').mkdir()
            (tree / 'sbin/init').write_bytes(b'\x7fELFfixture')
            image = root / 'root.img'
            subprocess.run([mke2fs, '-q', '-t', 'ext2', '-d', str(tree), '-F', str(image), '4096'],
                           check=True, capture_output=True)
            subprocess.run(['bash', str(existing.TOOLS.parent / 'write-boot-target.sh'), str(image), '--console'],
                           check=True, capture_output=True,
                           env=dict(os.environ, BREENIX_CONSOLE_IDLE_MINUTES='2', BREENIX_REQUIRE_DEBUGFS='1'))
            result = subprocess.run([debugfs, '-R', 'cat /etc/breenix/boot-target', str(image)],
                                    check=True, capture_output=True)
            self.assertEqual(result.stdout, b'console idle=2\n')


if __name__ == '__main__':
    unittest.main()
