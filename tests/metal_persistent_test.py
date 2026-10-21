"""The boot manager and persistent install's boot-loop side (docs/metal/README.md, "Boot manager and persistent
install"); no controller is contacted."""
import json
import os
from pathlib import Path
import struct
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch
import uuid

sys.path.insert(0, str(Path(__file__).resolve().parent))
import metal_tools_test as existing  # noqa: E402

m = existing.m
boot = existing.boot
http = existing.load_script('metal-http')
import metal_disk  # noqa: E402
import metal_persist  # noqa: E402
import metal_score  # noqa: E402


class LabAnswerTest(existing.StateTest):
    def lease(self, **fields):
        m.write_json(m.STATE / m.LEASE, dict(id='20261019T000000Z-metal', heartbeat=m.now(), **fields))

    def test_the_lease_decides_the_answer(self):
        self.lease(state='powered', kind='boot', mode='suite')
        self.assertEqual(http.manager_answer(), 'test 20261019T000000Z-metal')
        self.lease(state='powered', kind='console-session', mode='console')
        self.assertEqual(http.manager_answer(), 'test 20261019T000000Z-metal')
        self.lease(state='powered', kind='persistent', entry='previous')
        self.assertEqual(http.manager_answer(), 'persistent 20261019T000000Z-metal previous')
        self.lease(state='powered', kind='persistent', entry='current', slotAction='commit')
        self.assertEqual(http.manager_answer(), 'persistent 20261019T000000Z-metal current commit')
        for state in ('claimed', 'quarantined'):
            self.lease(state=state, kind='boot')
            self.assertEqual(http.manager_answer(), 'busy 20261019T000000Z-metal')

    def test_an_operators_power_on_becomes_a_persistent_session_only_on_the_persistent_layout(self):
        self.ready()
        with patch.object(http.subprocess, 'run', return_value=SimpleNamespace(returncode=0)) as start:
            self.assertIsNone(http.manager_answer())
            start.assert_not_called()
            metal_persist.update_disk(layout=metal_persist.new_layout())
            answer = http.manager_answer()
        kind, boot_id, entry = answer.split(' ')
        self.assertEqual((kind, entry), ('persistent', 'current'))
        lease = m.read_lease()
        self.assertEqual((lease['id'], lease['kind'], lease['state'], lease['unit']),
                         (boot_id, 'persistent', 'powered', 'metal-boot-' + boot_id))
        command = start.call_args.args[0]
        self.assertEqual(command[command.index('run'):], ['run', '--id', boot_id, '--payload', 'persistent', '--mode',
                                                          'persistent', '--adopt', '--wait', '240'])
        # A second question while the session holds the lease is answered from it.
        self.assertEqual(http.manager_answer(), f'persistent {boot_id} current')

    def test_a_failed_driver_start_leaves_no_lease(self):
        self.ready()
        metal_persist.update_disk(layout=metal_persist.new_layout())
        with patch.object(http.subprocess, 'run', return_value=SimpleNamespace(returncode=1)):
            self.assertIsNone(http.manager_answer())
        self.assertIsNone(m.read_lease())


class LayoutTest(existing.StateTest):
    def test_layout_partitions_are_ordered_aligned_and_recorded_beside_the_old_root(self):
        layout = metal_persist.new_layout()
        metal_persist.check_layout(layout)
        parts = [layout['partitions'][role] for role in metal_persist.ROLES]
        self.assertEqual([part['number'] for part in parts], [1, 2, 3, 4])
        self.assertTrue(all(part['first'] % 2048 == 0 for part in parts))
        self.assertTrue(all(a['last'] < b['first'] for a, b in zip(parts, parts[1:])))
        self.assertEqual(len({layout['diskGuid'], *(part['guid'] for part in parts)}), 5)
        m.write_json(m.STATE / 'disk.json', dict(installedBy='old', bytes=4096, sha256AfterRecord='a' * 64))
        self.assertEqual(metal_persist.test_root()['installedBy'], 'old')
        metal_persist.update_disk(layout=layout)
        self.assertEqual(metal_persist.layout(), layout)
        self.assertEqual(metal_persist.read_disk()['wholeDisk']['installedBy'], 'old')
        self.assertIsNone(metal_persist.test_root())
        metal_persist.record_test_root({'installedBy': 'new'})
        self.assertEqual(metal_persist.test_root(), {'installedBy': 'new'})
        self.assertEqual(metal_persist.layout(), layout)
        broken = dict(layout, partitions=dict(layout['partitions'], home=dict(layout['partitions']['home'], first=7)))
        with self.assertRaises(ValueError):
            metal_persist.check_layout(broken)

    def test_three_large_slots_fit_and_oversized_upgrades_fail_before_request_creation(self):
        with tempfile.TemporaryDirectory() as scratch:
            slot = Path(scratch) / 'slot'
            with slot.open('wb') as stream:
                stream.truncate(262 << 20)
            layout = metal_persist.new_layout()
            metal_persist.check_slot_capacity(layout, slot, {})
            small = dict(layout, partitions=dict(layout['partitions'], esp=dict(layout['partitions']['esp'])))
            small['partitions']['esp']['last'] = small['partitions']['esp']['first'] + (512 << 20) // 512 - 1
            with patch.object(metal_persist, 'Request') as request:
                with self.assertRaisesRegex(ValueError, 'CURRENT, PREVIOUS and NEXT'):
                    metal_persist.upgrade_request(scratch, small, {}, slot, slot, {})
                request.assert_not_called()

    def test_intents_and_the_tests_ramdisk(self):
        guid = str(uuid.uuid4())
        self.assertEqual(metal_persist.intent('default', root=guid),
                         f'BREENIX-BOOT 2\ntarget default\nroot {guid}\n'.encode())
        self.assertIn(b'userland install-42\n', metal_persist.intent('persistent', root=guid, userland='install-42'))
        self.assertIn(b'previous-userland install-41\n', metal_persist.intent('persistent-install', root=guid,
                                                                          userland='install-42', previous_userland='install-41'))
        for invalid in ('', '../old', 'old.new', 'x' * 65):
            with self.assertRaises(ValueError):
                metal_persist.intent('persistent', userland=invalid)
        with self.assertRaises(ValueError):
            metal_persist.intent('default', root='not-a-guid')
        with tempfile.TemporaryDirectory() as scratch:
            binaries = Path(scratch) / 'tests.img'
            binaries.write_bytes(b'BXTEST\0\0' + bytes(1016))
            ramdisk = metal_persist.tests_ramdisk(Path(scratch) / 'ramdisk', 'default', guid, binaries)
            data = ramdisk.read_bytes()
        self.assertTrue(data.startswith(f'BREENIX-BOOT 2\ntarget default\nroot {guid}\ntests 4096 1024\n'.encode()))
        self.assertEqual(data[4096:], b'BXTEST\0\0' + bytes(1016))

    def test_slot_image_carries_its_immutable_userland_identity(self):
        with tempfile.TemporaryDirectory() as scratch:
            layout = metal_persist.new_layout()
            files = dict.fromkeys(metal_persist.metal_image.ESP_FILES, Path(scratch) / 'unused')
            def assemble(output, contents):
                data = contents['ramdisk'].read_bytes()
                self.assertEqual(data, metal_persist.intent('persistent', root=layout['partitions']['system']['guid'],
                    home=layout['partitions']['home']['guid'], userland='install-42'))
                return output
            with patch.object(metal_persist.metal_image, 'assemble', side_effect=assemble):
                metal_persist.slot_image(Path(scratch) / 'slot', files, layout, scratch, 'install-42')

    def test_a_format_2_request_names_the_test_root_by_its_on_disk_guid(self):
        guid = str(uuid.uuid4())
        record = b'BREENIX-STORAGE 1\nroot x 4096 ' + b'a' * 64 + b'\n'
        record += bytes(512 - len(record))
        request = metal_disk.config_2(1, 4096, guid, bytes(512), record)
        magic, action, size, partition, expected, root = metal_disk.HEADER_2.unpack(request[:metal_disk.HEADER_2.size])
        self.assertEqual((magic, action, size), (b'BREENIX-SUITE-2\0', 1, 4096))
        self.assertEqual(partition, uuid.UUID(guid).bytes_le)
        self.assertEqual(root, record)
        # The stub reads the GUID at 32 and the two records after it (scripts/metal/efi-stub/src/suite_disk.rs).
        self.assertEqual(request[32:48], uuid.UUID(guid).bytes_le)
        self.assertEqual(request[48 + 512:48 + 1024], record)
        with self.assertRaises(ValueError):
            metal_disk.config_2(2, 4096, guid, bytes(512), record)


class RequestTest(unittest.TestCase):
    def test_extents_replayed_onto_a_blank_disk_give_back_the_image(self):
        with tempfile.TemporaryDirectory() as scratch:
            scratch = Path(scratch)
            image = scratch / 'image'
            blocks = [bytes(4096), b'\1' * 4096, bytes(4096), bytes(4096), b'\2' * 100 + bytes(3996), b'\3' * 4096]
            image.write_bytes(b''.join(blocks))
            request = metal_persist.Request(scratch, 'install')
            request.extents_of(2, image, [(0, len(blocks) * 8)])
            config = request.finish()
            lines = [line for line in config.read_text().splitlines() if line.startswith('extent ')]
            # Adjacent pieces of one kind merge: blank, data, two blank, two data.
            self.assertEqual(lines, ['extent 2 0 8 zero', 'extent 2 8 8 data 0', 'extent 2 16 16 zero',
                                     'extent 2 32 16 data 4096'])
            payload = request.data.read_bytes()
            disk = bytearray(len(blocks) * 4096)
            for line in lines:
                _, _, first, count, kind, *rest = line.split(' ')
                first, count = int(first), int(count)
                if kind == 'data':
                    offset = int(rest[0])
                    disk[first * 512:(first + count) * 512] = payload[offset:offset + count * 512]
            self.assertEqual(bytes(disk), image.read_bytes())
            self.assertEqual(request.written, len(blocks) * 4096)


class DriverPersistentTest(unittest.TestCase):
    def setUp(self):
        # The driver fixture, without inheriting (and so rerunning) its own tests.
        self.fixture = existing.DriverTest()
        self.fixture.setUp()
        self.driver = self.fixture.driver

    def tearDown(self):
        self.fixture.tearDown()

    def test_power_on_skips_the_override_on_the_persistent_layout(self):
        d = self.driver
        self.fixture.ready()
        m.claim_lease({'id': d.id, 'pid': os.getpid()}, wait=False)
        metal_persist.update_disk(layout=metal_persist.new_layout())
        with patch.object(m, 'rf', return_value={'startedAt': 105}) as rf, patch.object(m, 'rf_get') as rf_get:
            d.power_on()
        self.assertEqual([call.args[0] for call in rf.call_args_list], ['POST'])
        self.assertEqual(rf.call_args.args[2], {'ResetType': 'On'})
        rf_get.assert_not_called()

    def test_slot_lines_move_the_recorded_slots(self):
        d = self.driver
        current, nxt, previous = 'c' * 40, 'a' * 40, 'b' * 40
        # The first commit after the one-time install has no previous kernel to keep.
        metal_persist.update_disk(slots={'next': {'commit': previous}})
        d.on_slot_line(f'breenix-manager: slot commit: current {previous}, previous empty', 0)
        self.assertEqual(metal_persist.read_disk()['slots'], {'current': {'commit': previous}})
        metal_persist.update_disk(slots={'current': {'commit': previous}, 'next': {'commit': nxt}})
        d.on_slot_line(f'breenix-manager: slot commit: current {nxt}, previous {previous}', 1)
        slots = metal_persist.read_disk()['slots']
        self.assertEqual((slots['current']['commit'], slots['previous']['commit']), (nxt, previous))
        self.assertNotIn('next', slots)
        d.on_slot_line(f'breenix-manager: slot rollback: current {previous}, previous {nxt}', 2)
        slots = metal_persist.read_disk()['slots']
        self.assertEqual((slots['current']['commit'], slots['previous']['commit']), (previous, nxt))
        with self.assertRaises(boot.Finish):
            d.on_slot_line(f'breenix-manager: slot rollback: current {current}, previous {nxt}', 3)

    def test_successful_rollback_is_consumed_and_repeated_confirmation_does_not_swap_again(self):
        d = self.driver
        d.args.payload = 'persistent'
        a, b = 'a' * 40, 'b' * 40
        metal_persist.update_disk(slots={'current': {'commit': a}, 'previous': {'commit': b}})
        m.write_json(m.STATE / m.LEASE, dict(id=d.id, state='powered', kind='persistent',
                                            entry='current', slotAction='rollback'))
        line = f'breenix-manager: slot rollback: current {b}, previous {a}'
        d.on_slot_line(line, 1)
        self.assertEqual(http.manager_answer(), f'persistent {d.id} current')
        d.on_slot_line(line, 2)
        self.assertEqual(metal_persist.read_disk()['slots']['current']['commit'], b)
        self.assertIsNone(m.read_lease()['slotAction'])

    def test_actual_previous_slot_changes_the_attributed_commit_and_definition(self):
        d = self.driver
        d.args.payload = 'persistent'
        a, b = 'a' * 40, 'b' * 40
        d.args.commit = a
        metal_persist.update_disk(slots={'current': {'commit': a}, 'previous': {'commit': b}})
        source = m.STATE / 'persistent' / b / 'definition'
        source.mkdir(parents=True)
        (source / 'identity').write_text(b)
        (d.dir / 'definition').mkdir(exist_ok=True)
        with patch.object(d, 'load_definition') as load:
            d.on_slot_boot(f'breenix-manager: booting previous {b}', 1)
            load.assert_called_once()
        self.assertEqual((d.dir / 'commit').read_text(), b + '\n')
        self.assertEqual((d.dir / 'definition/identity').read_text(), b)
        self.assertEqual(m.read_json(d.dir / 'run.json')['bootedCommit'], b)
        self.assertIn('manager-slot', d.seen)
        with self.assertRaises(boot.Finish):
            d.on_slot_boot(f'breenix-manager: booting current {b}', 2)

    def test_run_logs_in_types_each_command_at_a_fresh_prompt_and_logs_out(self):
        d = self.driver
        d.args.payload, d.args.mode, d.args.run = 'persistent', 'persistent', ['writeFile("/root/a", "1")', 'sync']
        d.site = {'CONSOLE_USER': 'root', 'CONSOLE_PASSWORD': 'fixture'}
        d.kernel_started, d.t0 = 1, 0
        sent = []
        d.console_send = lambda text, paced=False: sent.append(text)
        offset = 0

        def prompt(text):
            nonlocal offset
            offset += 100
            d.line_start = offset
            d.challenge_persistent(text)
            d.challenge_persistent(text)  # the same partial line again is not a new prompt
        for text in ('Breenix login: ', 'Password: ', 'bsh /root> ', 'bsh /root> writeF', 'bsh /root> writeFile(',
                     'bsh /root> ', 'bsh /root> ', 'bsh /root> sy', 'bsh /root> ', 'Breenix login: '):
            # Redraws of a line being typed are prompts with text after them, not fresh ones.
            prompt(text)
        self.assertEqual(sent, ['root', 'fixture', 'writeFile("/root/a", "1")', 'sync', 'sync', 'exit'])
        self.assertTrue(d.run_done)
        d.run_done, d.run_step, d.sent_at = False, 2, -1
        with self.assertRaises(boot.Finish):
            d.challenge_persistent('Login incorrect')

    def test_run_refuses_a_reported_command_failure(self):
        d = self.driver
        d.args.payload, d.args.mode, d.args.run = 'persistent', 'persistent', ['false']
        d.kernel_started, d.run_step = 1, 3
        with self.assertRaises(boot.Finish) as result:
            d.challenge_persistent('[bsh] command failed status=1')
        self.assertEqual(result.exception.status, m.EXIT_SHORT)
        self.assertFalse(d.run_done)

    def test_run_answers_a_prompt_that_a_log_line_completed(self):
        d = self.driver
        d.args.payload, d.args.mode, d.args.run = 'persistent', 'persistent', ['true']
        d.site = {'CONSOLE_USER': 'root', 'CONSOLE_PASSWORD': 'fixture'}
        d.kernel_started, d.t0 = 1, 0
        sent = []
        d.console_send = lambda text, paced=False: sent.append(text)
        d.dir.mkdir(parents=True, exist_ok=True)
        (d.dir / 'serial.txt').touch()
        (d.dir / 'serial.ts').touch()
        # The prompt and the census line after it arrive in one read: the prompt is a complete line.
        raw = d.dir / 'serial.raw'
        raw.write_bytes(b'Breenix login: \r\n[census] cpu0=idle\r\n')
        d.pump_serial()
        self.assertEqual(sent, ['root'])


class OperatorScoringTest(existing.StateTest):
    def test_keyboard_stages_count_once_typed_and_are_unmeasured_until_then(self):
        directory = m.STATE / 'boot'
        definition = directory / 'definition'
        stages = [{'name': 'attached', 'marker': '[usb] boot keyboard attached: '},
                  {'name': 'typed', 'marker': '[hid] first key report', 'input': 'keyboard'},
                  {'name': 'delivered', 'marker': '[tty] first key delivered', 'input': 'keyboard'}]
        m.write_json(definition / 'docs/metal/boot-path.json', {'gateArch': 'x86_64', 'milestones': [
            {'id': 'usb-keyboard', 'bootShapes': ['persistent'], 'stages': {'persistent': stages}}]})
        m.write_json(definition / 'tools/breenix-runs/Sources/BreenixRuns/Resources/boot-stages-x86_64.json',
                     {'stages': []})
        (directory / 'events.jsonl').write_text('')
        m.write_json(directory / 'run.json', {'disk': 'persistent', 'mode': 'persistent', 'input': 'operator',
                                              'kind': 'persistent'})
        (directory / 'serial.txt').write_text('[usb] boot keyboard attached: port 1\n')
        verdict = metal_score.score(directory)
        self.assertEqual(verdict['milestones']['usb-keyboard'],
                         {'reached': 1, 'total': 3, 'passed': False, 'inputUnmeasured': 2})
        self.assertFalse(metal_score.falls_short(verdict))
        (directory / 'serial.txt').write_text('[usb] boot keyboard attached: port 1\n[hid] first key report\n')
        verdict = metal_score.score(directory)
        self.assertEqual(verdict['milestones']['usb-keyboard'], {'reached': 2, 'total': 3, 'passed': False})
        self.assertTrue(metal_score.falls_short(verdict))
        with (directory / 'serial.txt').open('a') as serial:
            serial.write('[tty] first key delivered\n')
        verdict = metal_score.score(directory)
        self.assertEqual(verdict['milestones']['usb-keyboard'], {'reached': 3, 'total': 3, 'passed': True})


if __name__ == '__main__':
    unittest.main()
