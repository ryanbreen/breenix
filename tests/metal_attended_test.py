"""Attended provenance/scoring and the real driver's timeout shutdown path."""
import json
from pathlib import Path
import sys
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
import metal_tools_test as existing
import metal_attended
import metal_score

run = existing.load_script('metal-run')

boot, m = existing.boot, existing.m

TRANSCRIPT = [
    '[init] getty on the console', '[getty] console acquired', 'Breenix login: ',
    '[console] first input byte input=keyboard',
    '[console] login refused input=keyboard', 'Login incorrect',
    '[getty] console acquired', 'Breenix login: ',
    '[console] login accepted input=keyboard',
    'print("metal-console-ok", 6*7)', 'metal-console-ok 42',
    '[init] console session ended; restarting getty',
    '[console] session ended input=keyboard', '[getty] console acquired', 'Breenix login: ',
]


class ScorerTest(unittest.TestCase):
    def score(self, lines):
        challenge = metal_attended.Challenge()
        for line in lines:
            challenge.feed(line)
        return challenge

    def test_usb_provenance_and_the_entire_ordered_challenge_are_required(self):
        self.assertTrue(self.score(TRANSCRIPT).done)
        for index in (4, 5, 8, 10, 11, 12, 13, 14):
            self.assertFalse(self.score(TRANSCRIPT[:index] + TRANSCRIPT[index + 1:]).done, index)
        self.assertFalse(self.score([TRANSCRIPT[10]] + TRANSCRIPT[:10] + TRANSCRIPT[11:]).done)

    def test_captured_prompt_includes_username_echo_and_live_partial_prompt_is_idempotent(self):
        lines = TRANSCRIPT.copy()
        lines[2] = 'Breenix login: root'
        lines[7] = 'Breenix login: root'
        self.assertTrue(self.score(lines).done)
        # SOL may deliver the prompt and typed name together or in fragments.
        challenge = metal_attended.Challenge()
        for line in lines[:2]:
            challenge.feed(line)
        challenge.feed('Breenix login: ')
        challenge.feed('Breenix login: ro')
        for line in lines[2:]:
            challenge.feed(line)
        self.assertTrue(challenge.done)

    def test_mixed_or_sol_input_and_repeated_refusal_cannot_pass(self):
        for marker in (4, 8, 12):
            lines = TRANSCRIPT.copy()
            lines[marker] = lines[marker].replace('input=keyboard', 'input=other')
            self.assertFalse(self.score(lines).done)
        self.assertFalse(self.score(TRANSCRIPT[:8] + TRANSCRIPT[4:6] + TRANSCRIPT[8:]).done)
        self.assertFalse(self.score(['Login incorrect', 'metal-console-ok 42', 'Breenix login: ']).done)


class AttendedTest(existing.StateTest):
    def setUp(self):
        super().setUp()
        self.args = SimpleNamespace(id='20261018T000001Z-metal', mode='console', payload='image',
                                    session=False, attended=True, wait=0.1, input=None, disk=None,
                                    target='branch:fixture', task='', commit='a' * 40, inject=None, nmi=False)
        with patch.object(m, 'load_site', return_value={}):
            self.driver = boot.Driver(self.args)
        self.driver.event, self.driver.phase = Mock(), Mock()
        self.driver.dir.mkdir(parents=True)
        self.driver.t0 = 100
        self.driver.kernel_started = 110

    def test_deadline_boundary_is_clean_not_a_hang_and_driver_never_types(self):
        d = self.driver
        d.console_send = Mock()
        for line in ['Breenix login: ', 'Password: ', 'bsh /root>']:
            d.challenge_console(line)
        d.console_send.assert_not_called()
        with patch.object(m, 'now', return_value=115.999):
            d.check_deadlines()
        with patch.object(m, 'now', return_value=116):
            with self.assertRaises(boot.Finish) as result:
                d.check_deadlines()
        self.assertEqual(result.exception.status, m.EXIT_SHORT)
        self.assertFalse(result.exception.hung)
        self.assertIn('not reached', result.exception.reason)
        self.assertEqual(boot.ceiling_seconds(SimpleNamespace(mode='console', attended=True, wait=30)), 2700)

    def test_partial_marker_cannot_duplicate_refusal_and_prompt_finishes_without_newline(self):
        d = self.driver
        d.required_stages, d.final_seen = [], True
        d.console_send = Mock()
        for line in TRANSCRIPT[:-1]:
            d.challenge_console(line)  # partial, repeated as SOL buffers arrive
            d.challenge_console(line, complete=True)
        with self.assertRaises(boot.Finish) as result:
            d.challenge_console(TRANSCRIPT[-1])
        self.assertEqual(result.exception.status, m.EXIT_DONE)
        d.console_send.assert_not_called()
        d.args.attended = True
        with self.assertRaises(RuntimeError), patch.object(boot.subprocess, 'run') as send:
            boot.Driver.console_send(d, 'anything')
        send.assert_not_called()

    def test_timeout_retains_scored_boot_and_calls_power_off(self):
        d = self.driver
        d.dir.rmdir()
        d.create_record()
        record = json.loads((d.dir / 'run.json').read_text())
        self.assertEqual(record['kind'], 'boot')
        self.assertTrue(record['evidenceEligible'])
        self.assertEqual(record['input'], 'keyboard')
        definition = d.dir / 'definition/docs/metal/boot-path.json'
        definition.parent.mkdir(parents=True)
        definition.write_text(json.dumps({'gateArch': 'x86_64', 'milestones': [
            {'id': 'console-login', 'stages': {'console': []}}]}))
        catalog = d.dir / 'definition/tools/breenix-runs/Sources/BreenixRuns/Resources/boot-stages-x86_64.json'
        catalog.parent.mkdir(parents=True)
        catalog.write_text('{"stages": []}')
        (d.dir / 'serial.txt').write_text('\n'.join(TRANSCRIPT[:3]))
        d.create_record = d.claim = d.prepare_payload = d.preflight = d.do_stage = d.start_console = d.power_on = Mock()
        d.watch = lambda: d.check_deadlines()
        d.power_off, d.cleanup = Mock(return_value=True), Mock()
        with patch.object(m, 'now', return_value=116):
            self.assertEqual(d.run(), m.EXIT_SHORT)
        d.power_off.assert_called_once_with(False)
        verdict = json.loads((d.dir / 'milestone-verdict.json').read_text())
        self.assertEqual(verdict['milestones']['console-login'], {'reached': 2, 'total': 8, 'passed': False})

    def test_keyboard_final_marker_and_required_stages_allow_completion(self):
        d = self.driver
        d.final_marker = ('[console] first input byte input=keyboard', 'line', 'keyboard')
        d.required_stages = [[d.final_marker]]
        for line in TRANSCRIPT[:-1]:
            d.on_line(line)
        self.assertTrue(d.final_seen)
        self.assertEqual(d.required_stages, [[]])
        with self.assertRaises(boot.Finish) as result:
            d.challenge_console(TRANSCRIPT[-1])
        self.assertEqual(result.exception.status, m.EXIT_DONE)

    def test_cli_forwards_attended_wait_without_sol_input(self):
        args = SimpleNamespace(**vars(self.args), sha='a' * 40, branch=None, no_wait=False,
                               linux_directory=None, serve_as=None, content_type=None, ramdisk=True, suite=None)
        with patch.object(run, 'host_env', return_value={}), patch.object(run, 'verify_target'), \
             patch.object(run, 'in_metal', return_value=SimpleNamespace(returncode=75, stdout='', stderr='busy')) as launch:
            self.assertEqual(run.boot(args), 75)
        command = launch.call_args.args[1:]
        self.assertIn('--attended', command)
        self.assertEqual(command[command.index('--wait') + 1], '0.1')
        self.assertNotIn('--input', command)
        for invalid in (0, -1, float('nan'), float('inf'), 241):
            args.wait = invalid
            with self.assertRaises(SystemExit):
                run.boot(args)

    def test_tty_stage_needs_kernel_source_marker_even_when_record_says_keyboard(self):
        definition = {'milestones': [{'id': 'usb-keyboard', 'stages': {'console': [
            {'name': 'Keys reached the console TTY', 'marker': '[hid] typed into the console tty: '}]}}]}
        selected = metal_score.attended_definition(definition)
        spec = selected['milestones'][0]['stages']['console'][0]
        self.assertEqual(spec['marker'], '[console] first input byte input=keyboard')
        self.assertEqual(spec['match'], 'line')
        self.assertEqual(spec['input'], 'keyboard')
        self.assertEqual(definition['milestones'][0]['stages']['console'][0]['marker'], '[hid] typed into the console tty: ')


if __name__ == '__main__':
    unittest.main()
