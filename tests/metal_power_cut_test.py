"""Drive deliberate power cuts through the boot loop with an offline serial capture."""
from contextlib import redirect_stderr, redirect_stdout
import io
import sys
from types import SimpleNamespace
import unittest
from unittest.mock import patch

from metal_tools_test import StateTest, TOOLS, boot, load_script, m


CLI = ['--suite', 'filesystem', '--disk', 'root', '--root-fs', 'breenfs', '--case', 'power-cut/writer']
DRIVER = ['--mode', 'suite', *CLI]
MARKER = '[breenfs] commit-window first-flush'


class PowerCutCliTest(unittest.TestCase):
    def test_forward_each_trigger_to_driver(self):
        run = load_script('metal-run')
        for flag, value in (('--power-cut-after', '12.5'), ('--power-cut-window', 'first-flush')):
            with self.subTest(flag=flag):
                args = run.argument_parser().parse_args(['boot', *CLI, flag, value])
                refusal = SimpleNamespace(returncode=1, stdout='', stderr='fixture')
                with patch.object(run, 'host_env', return_value={}), \
                     patch.object(run, 'resolve', return_value='a' * 40), \
                     patch.object(run, 'resolve_suites', return_value='filesystem'), \
                     patch.object(run, 'in_metal', return_value=refusal) as remote, \
                     redirect_stdout(io.StringIO()), redirect_stderr(io.StringIO()):
                    self.assertEqual(run.boot(args), 1)
                command = remote.call_args.args
                self.assertEqual(command[command.index(flag) + 1], value)
                with patch.object(sys, 'argv', list(command[2:])), \
                     patch.object(boot, 'launch', return_value=0) as launch:
                    self.assertEqual(boot.main(), 0)
                parsed = launch.call_args.args[0]
                expected = float(value) if flag == '--power-cut-after' else value
                self.assertEqual(getattr(parsed, flag[2:].replace('-', '_')), expected)

    def test_fault_launch_selects_matching_feature_and_intent(self):
        run = load_script('metal-run')
        for flags, feature, intent in (
            (['--power-cut-window', 'node-writes'], 'breenfs_commit_window',
             'breenfs_commit_window=node-writes breenfs_after_receipt=1'),
            (['--breenfs-bit-flip'], 'breenfs_corrupt_inject',
             'breenfs_corrupt_inject=first-data:64:0'),
        ):
            with self.subTest(flags=flags):
                args = run.argument_parser().parse_args(
                    ['boot', *CLI, *flags])
                args.mode = 'suite'
                run.metal_power_cut.validate(args)
                self.assertEqual(run.metal_power_cut.kernel_fault(args), (feature, intent))
                self.assertEqual(run.argument_parser().parse_args(
                    ['_build', 'id', 'a' * 40, '--features', feature]).features, feature)
                args.kernel_command_line = 'breenfs_commit_window=between-rings'
                with self.assertRaises(ValueError):
                    run.metal_power_cut.validate(args)

    def test_reject_unsafe_or_ambiguous_requests_before_launch(self):
        run = load_script('metal-run')
        parser = run.argument_parser()
        cases = (
            (['--power-cut-after', '0'], CLI),
            (['--power-cut-after', 'inf'], CLI),
            (['--power-cut-after', 'nan'], CLI),
            (['--power-cut-window', 'unknown'], CLI),
            (['--power-cut-window', 'node-writes', '--power-cut-after', '5'], CLI),
            (['--power-cut-after', '5'], []),
            (['--power-cut-after', '5', '--persistent'], []),
            (['--power-cut-after', '5', '--nmi'], CLI),
            (['--power-cut-after', '5', '--inject', 'forceoff-fail'], CLI),
        )
        for trigger, options in cases:
            with self.subTest(trigger=trigger, options=options):
                with patch.object(run, 'host_env', side_effect=AssertionError('must refuse before launch')), \
                     redirect_stderr(io.StringIO()), self.assertRaises(SystemExit):
                    run.boot(parser.parse_args(['boot', *options, *trigger]))

        with patch.object(sys, 'argv', ['metal-boot', 'launch', *DRIVER, '--power-cut-after', '5', '--nmi']), \
             patch.object(boot, 'launch', side_effect=AssertionError('must refuse before launch')), \
             redirect_stderr(io.StringIO()), self.assertRaises(SystemExit):
            boot.main()


class PowerCutDriverTest(StateTest):
    def driver(self, **changes):
        values = dict(
            id='20261121T000001Z-metal', mode='suite', suite='filesystem', payload='image',
            disk='root', root_fs='breenfs', target='branch:fixture', task='', commit='a' * 40,
            inject=None, power_cut_after=None, power_cut_window=None,
        )
        args = SimpleNamespace(**(values | changes))
        with patch.object(m, 'load_site', return_value={}):
            return boot.Driver(args)

    def test_match_complete_marker_and_record_cut_before_cleanup(self):
        driver = self.driver(power_cut_window='first-flush')
        driver.t0 = 90.0
        driver.kernel_started = 92.0
        driver.suite_name = 'filesystem'
        driver.seen.add('disk-root-confirmed')
        # Replay the beginning of a retained metal capture with the u2 markers
        # inserted as the new producer does not exist at this base.
        serial = TOOLS.parents[1] / 'tests/fixtures/metal-suite-verdict/serials/20261105T043719Z-metal.txt'
        excerpt = serial.read_bytes().splitlines()[2:5]
        raw = driver.dir / 'serial.raw'

        def watch():
            raw.write_bytes(b'\r\n'.join(excerpt + [
                b'',
                b'SUITE filesystem START cases=1 only=power-cut/writer nonce=0123456789abcdef',
                b'FSYNC 0123456789abcdef 1 ' + b'a' * 64,
                b'[breenfs] commit-window node-writes',
                b'echo ' + MARKER.encode(),
                MARKER.encode() + b' trailing text',
                MARKER[:20].encode(),
            ]))
            driver.pump_serial()
            with raw.open('ab') as stream:
                stream.write(MARKER[20:].encode() + b'\r\n')
            driver.pump_serial()
            self.fail('the exact serial marker did not cut power')

        def force_off(*, immediate):
            self.assertTrue(immediate)
            self.assertIn(MARKER + '\n', (driver.dir / 'serial.txt').read_text())
            after = driver.dir / 'serial.raw'
            with after.open('ab') as stream:
                stream.write(b'tail from the serial capture\r\n')
            return True

        with patch.multiple(driver, claim=lambda: None, prepare_payload=lambda: None,
                            preflight=lambda: None, do_stage=lambda: None,
                            start_console=lambda: None, power_on=lambda: None, watch=watch,
                            continue_disk_readback=lambda: m.EXIT_SHORT), \
             patch.object(m, 'now', return_value=100.0), \
             patch.object(m, 'rf', side_effect=AssertionError('use the existing power-off path')), \
             patch.object(m, 'force_off_and_wait', side_effect=force_off) as off:
            self.assertEqual(driver.run(), m.EXIT_SHORT)
        off.assert_called_once_with(immediate=True)
        record = m.read_json(driver.dir / 'run.json')
        self.assertFalse(record['evidenceEligible'])
        self.assertEqual(record['powerCut']['window'], 'first-flush')
        self.assertEqual(record['powerCut']['triggerLine'], MARKER)
        self.assertEqual(record['powerCut']['cutAt'], 100.0)
        self.assertEqual(record['powerCut']['triggerObservedAt'], 100.0)
        self.assertEqual(record['powerCut']['powerOffAt'], 100.0)
        self.assertTrue(record['powerCut']['powerOffConfirmed'])
        self.assertIn('tail from the serial capture\n', (driver.dir / 'serial.txt').read_text())

    def test_aimed_cut_waits_for_the_writers_first_acknowledgement(self):
        driver = self.driver(power_cut_window='first-flush')
        driver.t0 = 90.0
        driver.kernel_started = 92.0
        driver.seen.add('disk-root-confirmed')
        nonce = '0123456789abcdef'
        driver.on_line(f'SUITE filesystem START cases=1 only=power-cut/writer nonce={nonce}')
        driver.on_line(MARKER)
        driver.on_line(f'FSYNC {nonce} 1 ' + 'a' * 64)
        with self.assertRaises(boot.PowerCut):
            driver.on_line(MARKER)

    def test_immediate_off_posts_before_querying_power_state(self):
        requests = []
        with patch.object(m, 'rf', side_effect=lambda *args, **kw: requests.append(args[0])), \
             patch.object(m, 'power_state', side_effect=lambda **kw: requests.append('GET') or 'Off'):
            self.assertTrue(m.force_off_and_wait(immediate=True))
        self.assertEqual(requests, ['POST', 'GET'])

    def test_elapsed_seconds_trigger_at_power_on_deadline(self):
        driver = self.driver(power_cut_after=12.5)
        driver.t0 = 100.0
        with patch.object(m, 'now', return_value=112.499):
            driver.check_power_cut_deadline()
        with patch.object(m, 'now', return_value=112.5), self.assertRaises(boot.Finish):
            driver.check_power_cut_deadline()


if __name__ == '__main__':
    unittest.main()
