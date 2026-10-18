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

DEFINITION = json.loads((existing.TOOLS.parents[1] / 'docs/metal/boot-path.json').read_text())
STAGES = metal_attended.stages(DEFINITION)

# One wrong attempt, then the real one; every line below the first prompt is one the challenge needs.
TRANSCRIPT = [
    '[init] getty on the console', '[getty] console acquired', 'Breenix login: root',
    '[console] first input byte input=keyboard',
    '[console] login refused input=keyboard', 'Login incorrect',
    '[init] getty on the console', '[getty] console acquired', 'Breenix login: root',
    '[console] login accepted input=keyboard',
    'print("metal-login-ok", 6*7)', 'metal-login-ok 42',
    'fbcon check: shadow and screen identical, 412 glyph cells match the font',
    'fbcon row: bsh /root> print("metal-login-ok", 6*7)', 'fbcon row: metal-login-ok 42',
    '[init] console session ended; restarting getty',
    '[console] session ended input=keyboard', '[getty] console acquired', 'Breenix login: ',
]
NEEDED = (4, 5, 9, 11, 12, 14, 15, 16, 17, 18)


class ScorerTest(unittest.TestCase):
    def score(self, lines):
        challenge = metal_attended.Challenge(STAGES)
        for line in lines:
            challenge.feed(line)
        return challenge

    def test_usb_provenance_screen_check_and_the_entire_ordered_challenge_are_required(self):
        self.assertTrue(self.score(TRANSCRIPT).done)
        for index in NEEDED:
            self.assertFalse(self.score(TRANSCRIPT[:index] + TRANSCRIPT[index + 1:]).done, index)
        # The answer before the accepted login cannot count.
        self.assertFalse(self.score(TRANSCRIPT[:9] + [TRANSCRIPT[11]] + TRANSCRIPT[9:11] + TRANSCRIPT[12:]).done)

    def test_any_number_of_refusals_may_precede_the_login_and_one_must(self):
        refused = TRANSCRIPT[4:9]
        self.assertTrue(self.score(TRANSCRIPT[:4] + refused * 3 + TRANSCRIPT[9:]).done)
        accepted_first = TRANSCRIPT[:4] + TRANSCRIPT[9:]
        self.assertFalse(self.score(accepted_first).done)
        # A first session without a refusal before it is not the proof; a later refusal and login are.
        self.assertTrue(self.score(accepted_first[:-1] + TRANSCRIPT[4:]).done)

    def test_prompt_with_echo_and_live_partial_prompt_are_idempotent(self):
        challenge = metal_attended.Challenge(STAGES)
        for line in TRANSCRIPT[:2]:
            challenge.feed(line)
        for text in ('Breenix login: ', 'Breenix login: ', 'Breenix login: ro', 'Breenix login: root'):
            challenge.feed(text)
        for line in TRANSCRIPT[3:]:
            challenge.feed(line)
        self.assertTrue(challenge.done)

    def test_non_keyboard_input_invalidates_the_challenge(self):
        for index in (4, 9, 16):
            lines = TRANSCRIPT.copy()
            lines[index] = lines[index].replace('input=keyboard', 'input=other')
            self.assertFalse(self.score(lines).done, index)
        lines = TRANSCRIPT[:4] + ['[console] login refused input=other'] + TRANSCRIPT[4:]
        verdict = self.score(lines).verdict()
        self.assertEqual((verdict['reached'], verdict['passed']), (0, False))
        self.assertIn('invalid', verdict)

    def test_the_first_local_login_names_the_attended_computation(self):
        first = next(f for f in DEFINITION['firsts'] if f['id'] == 'first-local-login')
        stage = next(s for s in STAGES if s['name'] == first['stage'])
        self.assertEqual(stage['marker'], 'metal-login-ok 42')
        login = next(m for m in DEFINITION['milestones'] if m['id'] == 'console-login')
        sol = next(s for s in login['stages']['console'] if s['name'] == first['stage'])
        self.assertEqual(sol['marker'], stage['marker'])


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
        self.driver.attended_challenge = metal_attended.Challenge(STAGES)

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
            {'id': 'console-login', 'stages': {'console': []}, 'attendedStages': STAGES}]}))
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
        self.assertEqual(verdict['milestones']['console-login'],
                         {'reached': 2, 'total': len(STAGES), 'passed': False,
                          'missing': [stage['name'] for stage in STAGES[2:]]})

    def test_attended_keeps_hang_detection_and_ends_on_non_keyboard_input_or_idle(self):
        d = self.driver
        d.args.wait = 30
        d.request_at = d.first_byte_at = d.complete_at = 105
        d.required_stages = [[('[usb] boot keyboard attached: ', 'contains')]]
        d.last_serial = 110
        with patch.object(m, 'now', return_value=110 + boot.SILENCE_SECONDS + 1):
            with self.assertRaises(boot.Finish) as result:
                d.check_deadlines()
        self.assertTrue(result.exception.hung)
        d.last_serial = 110 + boot.MODE_DEADLINE_SECONDS['console']
        with patch.object(m, 'now', return_value=111 + boot.MODE_DEADLINE_SECONDS['console']):
            with self.assertRaises(boot.Finish) as result:
                d.check_deadlines()
        self.assertEqual(result.exception.status, m.EXIT_SHORT)
        self.assertIn('mode deadline', result.exception.reason)
        # Only typed stages left and the prompt reached: the operator's wait governs.
        d.required_stages = [[('[hid] first key report', 'contains', 'keyboard')]]
        for line in TRANSCRIPT[:3]:
            d.attended_challenge.feed(line)
        with patch.object(m, 'now', return_value=111 + boot.MODE_DEADLINE_SECONDS['console']):
            d.check_deadlines()
        with self.assertRaises(boot.Finish) as result:
            d.on_line('[console] input idle 30 min')
        self.assertEqual(result.exception.status, m.EXIT_SHORT)
        with self.assertRaises(boot.Finish) as result:
            d.challenge_console('[console] login refused input=other', complete=True)
        self.assertEqual(result.exception.status, m.EXIT_SHORT)

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


if __name__ == '__main__':
    unittest.main()
