#!/usr/bin/env python3
"""Host tests for the x86-metal boot loop's tools (scripts/metal): no machine, controller or container needed."""
import hashlib
import importlib.machinery
import importlib.util
import json
import os
from pathlib import Path
import re
import socket
import socketserver
import struct
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from unittest.mock import patch, Mock
from types import SimpleNamespace
import urllib.error
import urllib.request
import uuid

TOOLS = Path(__file__).resolve().parents[1] / 'scripts' / 'metal'
FIXTURES = TOOLS / 'fixtures'
sys.path.insert(0, str(TOOLS))
import metal_lib as m  # noqa: E402
import metal_image  # noqa: E402


def load_script(name):
    loader = importlib.machinery.SourceFileLoader(name.replace('-', '_'), str(TOOLS / name))
    spec = importlib.util.spec_from_loader(loader.name, loader)
    module = importlib.util.module_from_spec(spec)
    loader.exec_module(module)
    return module


machine = load_script('metal-machine')
boot = load_script('metal-boot')


class StateTest(unittest.TestCase):
    """Points the library's state and run folders at a private temporary directory."""

    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        root = Path(self.temporary.name)
        self.saved = (m.STATE, m.RUN, m.SITE_PATH)
        m.STATE, m.RUN = root / 'state', root / 'run'
        m.STATE.mkdir()
        m.RUN.mkdir()

    def tearDown(self):
        m.STATE, m.RUN, m.SITE_PATH = self.saved
        self.temporary.cleanup()

    def ready(self):
        m.write_json(m.RUN / 'ready.json', {'pid': os.getpid(), 'ready': m.now()})


class SerialCleanerTest(unittest.TestCase):
    def clean(self, *chunks):
        cleaner = m.SerialCleaner()
        return ''.join(cleaner.feed(chunk) for chunk in chunks)

    def test_line_endings(self):
        self.assertEqual(self.clean(b'one\r\ntwo\rthree\n'), 'one\ntwo\nthree\n')

    def test_escape_sequences_and_nuls_are_removed(self):
        self.assertEqual(self.clean(b'\x1b[1;37;40mBIOS\x1b[0m\x00 ok\n'), 'BIOS ok\n')
        self.assertEqual(self.clean(b'\x1b]0;title\x07\x1b(Bplain\n'), 'plain\n')

    def test_cursor_positioning_breaks_lines(self):
        self.assertEqual(self.clean(b'F2 = Setup\x1b[5;1HF11 = Boot Manager\x1b[2J\n'),
                         'F2 = Setup\nF11 = Boot Manager\n')

    def test_sequences_and_characters_split_across_reads(self):
        self.assertEqual(self.clean(b'a\x1b[3', b'1mb\xe2\x80', b'\x94c\n'), 'ab—c\n')

    def test_backspace_edits_the_line(self):
        cleaner = m.SerialCleaner()
        cleaner.feed(b'abc\b')
        self.assertEqual(cleaner.line, 'ab')


class RedactorTest(unittest.TestCase):
    def setUp(self):
        self.site = m.load_site(FIXTURES / 'redact-site.env')
        self.redact = m.Redactor(self.site)

    def test_fixture_matches_expected(self):
        text = (FIXTURES / 'redact-input.txt').read_text()
        self.assertEqual(self.redact(text), (FIXTURES / 'redact-expected.txt').read_text())

    def test_nothing_private_survives(self):
        out = self.redact((FIXTURES / 'redact-input.txt').read_text())
        for key, value in self.site.items():
            if key != 'REDACT_IDENTIFIERS':
                self.assertEqual(self.redact(value), '<site>')
        for name in m.identifier_list(self.site):
            self.assertNotIn(name, out)
        self.assertIsNone(re.search(r'\b\d{1,3}(\.\d{1,3}){3}\b', out))
        self.assertIsNone(re.search(r'([0-9A-Fa-f]{2}[:-]){5}[0-9A-Fa-f]{2}', out))
        self.assertNotIn('fe80', out)
        self.assertNotIn('/Users/', out)
        self.assertNotIn('/var/lib/breenix-metal', out)

    def test_ordinary_text_is_kept(self):
        for text in ('at 12:34:56', 'version 1.2.3', 'ratio 3:2', 'code 0x7f', '[timer] 1000 Hz',
                     'Breenix boot.img over DHCP on port 8240', 'HTTP device NIC.Embedded.2-1-1'):
            self.assertEqual(m.Redactor({})(text), text)

    def test_list_never_carries_a_credential(self):
        names = m.identifier_list(self.site)
        self.assertIn('fixtureauto', names)
        self.assertIn('FX12345', names)
        self.assertNotIn(self.site['IDRAC_PASSWORD'], names)
        listed = subprocess.run([sys.executable, str(TOOLS / 'redact'), '--site', str(FIXTURES / 'redact-site.env'),
                                 '--list'], capture_output=True, text=True, check=True).stdout
        self.assertNotIn(self.site['IDRAC_PASSWORD'], listed)
        self.assertIn('fixturehost', listed.split())

    def test_cli_redacts_stdin(self):
        out = subprocess.run([sys.executable, str(TOOLS / 'redact'), '--site', str(FIXTURES / 'redact-site.env')],
                             input='from 192.0.2.66 as fixtureauto\n', capture_output=True, text=True, check=True)
        self.assertEqual(out.stdout, 'from <site> as <site>\n')


class HttpServerTest(StateTest):
    def setUp(self):
        super().setUp()
        with socket.socket() as probe:
            probe.bind(('127.0.0.1', 0))
            self.port = probe.getsockname()[1]
        site = Path(self.temporary.name) / 'site.env'
        site.write_text(f'BOOT_URL=http://127.0.0.1:{self.port}/breenix/boot.img\nHTTP_LISTEN_PORT={self.port}\n')
        environment = dict(os.environ, BREENIX_METAL_SITE=str(site), BREENIX_METAL_STATE=str(m.STATE))
        self.server = subprocess.Popen([sys.executable, str(TOOLS / 'metal-http')], env=environment)
        deadline = time.time() + 10
        while time.time() < deadline:
            with socket.socket() as client:
                if client.connect_ex(('127.0.0.1', self.port)) == 0:
                    break
            time.sleep(0.05)
        self.url = f'http://127.0.0.1:{self.port}/breenix/boot.img'
        self.image = Path(self.temporary.name) / 'boot.img'
        self.image.write_bytes(os.urandom(3 << 20))
        self.digest = hashlib.sha256(self.image.read_bytes()).hexdigest()

    def tearDown(self):
        self.server.terminate()
        self.server.wait()
        super().tearDown()

    def request(self, method='GET', url=None):
        try:
            with urllib.request.urlopen(urllib.request.Request(url or self.url, method=method), timeout=10) as reply:
                return reply.status, dict(reply.headers), reply.read()
        except urllib.error.HTTPError as error:
            with error:
                return error.code, dict(error.headers), b''

    def log(self):
        return [json.loads(line) for line in (m.STATE / 'http.jsonl').read_text().splitlines()]

    def wait_complete(self):
        """The client holds the body before the server has recorded the transfer; wait for its log entry."""
        deadline = time.time() + 10
        while time.time() < deadline and not any(e['event'] == 'complete' for e in self.log()):
            time.sleep(0.02)

    def test_nothing_staged_is_404(self):
        self.assertEqual(self.request()[0], 404)
        self.assertEqual(self.request('HEAD')[0], 404)

    def test_one_complete_transfer_then_404(self):
        m.stage('20261017T000000Z-metal', self.image, self.digest, self.image.stat().st_size, 'application/vnd.efi-img')
        status, headers, _ = self.request('HEAD')
        self.assertEqual(status, 200)
        self.assertEqual(headers['Content-Type'], 'application/vnd.efi-img')
        self.assertEqual(int(headers['Content-Length']), 3 << 20)
        status, _, body = self.request()
        self.assertEqual((status, hashlib.sha256(body).hexdigest()), (200, self.digest))
        self.wait_complete()
        self.assertEqual(self.request()[0], 404)
        self.assertEqual(self.request('HEAD')[0], 404)
        complete = [entry for entry in self.log() if entry['event'] == 'complete']
        self.assertEqual(len(complete), 1)
        self.assertTrue(complete[0]['matches'])
        self.assertTrue(all('client' not in entry and 'address' not in entry for entry in self.log()))

    def test_a_broken_transfer_can_be_retried(self):
        m.stage('20261017T000000Z-metal', self.image, self.digest, self.image.stat().st_size, 'application/efi')
        with socket.create_connection(('127.0.0.1', self.port)) as client:
            client.sendall(b'GET /breenix/boot.img HTTP/1.1\r\nHost: x\r\n\r\n')
            client.recv(65536)
        deadline = time.time() + 10
        while time.time() < deadline and not any(e['event'] == 'broken' for e in self.log()):
            time.sleep(0.05)
        self.assertTrue(any(e['event'] == 'broken' for e in self.log()))
        status, _, body = self.request()
        self.assertEqual((status, hashlib.sha256(body).hexdigest()), (200, self.digest))
        self.wait_complete()
        self.assertEqual(self.request()[0], 404)

    def test_byte_ranges_count_once_the_whole_file_is_covered(self):
        m.stage('20261017T000000Z-metal', self.image, self.digest, self.image.stat().st_size, 'application/efi')
        size = self.image.stat().st_size
        data = self.image.read_bytes()
        parts = []
        for header in (f'bytes=0-{(1 << 20) - 1}', f'bytes={1 << 20}-'):
            request = urllib.request.Request(self.url, headers={'Range': header})
            with urllib.request.urlopen(request, timeout=10) as reply:
                self.assertEqual(reply.status, 206)
                parts.append(reply.read())
                self.assertIn('Content-Range', reply.headers)
        self.assertEqual(b''.join(parts), data)
        self.wait_complete()
        self.assertEqual(self.request()[0], 404)
        complete = [entry for entry in self.log() if entry['event'] == 'complete']
        self.assertEqual(len(complete), 1)
        self.assertTrue(complete[0]['matches'] and complete[0]['ranged'])
        self.assertEqual(size, 3 << 20)

    def test_an_unsatisfiable_range_is_416(self):
        m.stage('20261017T000000Z-metal', self.image, self.digest, self.image.stat().st_size, 'application/efi')
        request = urllib.request.Request(self.url, headers={'Range': f'bytes={4 << 20}-'})
        with self.assertRaises(urllib.error.HTTPError) as caught:
            urllib.request.urlopen(request, timeout=10)
        with caught.exception:
            self.assertEqual(caught.exception.code, 416)
        self.assertEqual(self.request()[0], 200)

    def test_other_paths_are_404(self):
        m.stage('20261017T000000Z-metal', self.image, self.digest, self.image.stat().st_size, 'application/efi')
        self.assertEqual(self.request(url=f'http://127.0.0.1:{self.port}/other')[0], 404)
        self.assertEqual(self.request()[0], 200)


class LeaseAndRecordTest(StateTest):
    def test_no_lease_before_reconciliation(self):
        with self.assertRaises(m.NotReady):
            m.claim_lease({'id': 'a', 'pid': os.getpid()}, wait=False)

    def test_one_holder_at_a_time(self):
        self.ready()
        m.claim_lease({'id': 'a', 'pid': os.getpid()}, wait=False)
        with self.assertRaises(m.Busy):
            m.claim_lease({'id': 'b', 'pid': os.getpid()}, wait=False)
        self.assertFalse(m.update_lease('b'))
        self.assertTrue(m.update_lease('a', phase='post'))
        self.assertFalse(m.release_lease('b'))
        self.assertTrue(m.release_lease('a'))
        m.claim_lease({'id': 'b', 'pid': os.getpid()}, wait=False)

    def test_quarantine_refuses_and_is_not_released_by_its_holder(self):
        self.ready()
        m.claim_lease({'id': 'a', 'pid': os.getpid()}, wait=False)
        m.quarantine_lease('a', 'power-off failed')
        with self.assertRaises(m.Quarantined):
            m.claim_lease({'id': 'b', 'pid': os.getpid()}, wait=True)
        self.assertFalse(m.release_lease('a'))
        m.update_lease('a', state='powered')
        self.assertEqual(m.read_lease()['state'], 'quarantined')

    def test_records_close_once_with_increasing_sequence(self):
        first, second = m.boot_dir('one'), m.boot_dir('two')
        first.mkdir(parents=True)
        second.mkdir(parents=True)
        self.assertTrue(m.close_record(first, 124, 'driver'))
        self.assertFalse(m.close_record(first, 0, 'reaper'))
        self.assertTrue(m.close_record(second, 137, 'reaper'))
        self.assertEqual((first / 'exit-status').read_text(), '124\n')
        self.assertEqual((second / 'ended-by').read_text(), 'reaper\n')
        entries = [json.loads(line) for line in (m.STATE / 'finished.jsonl').read_text().splitlines()]
        self.assertEqual(entries, [{'id': 'one', 'seq': 1}, {'id': 'two', 'seq': 2}])

    def test_post_budget(self):
        self.assertEqual(m.post_budget(), 600)
        for seconds in (106, 95, 120):
            m.record_post_time('x', seconds)
        self.assertEqual(m.post_budget(), 240)


class FakeController:
    """Answers the machine service's queue protocol for a machine with a power state."""

    def __init__(self, path, power='On'):
        self.power, self.forceoff_works, self.requests = power, True, []
        controller = self

        class Handler(socketserver.StreamRequestHandler):
            def handle(self):
                request = json.loads(self.rfile.readline())
                controller.requests.append(request)
                self.wfile.write(json.dumps(controller.answer(request)).encode() + b'\n')

        class Server(socketserver.ThreadingMixIn, socketserver.UnixStreamServer):
            daemon_threads = True
        self.server = Server(str(path), Handler)
        threading.Thread(target=self.server.serve_forever, daemon=True).start()

    def answer(self, request):
        if request['op'] == 'ipmi':
            if request['args'][:3] == ['chassis', 'power', 'off']:
                self.power = 'Off'
            return {'ok': True, 'output': ''}
        path, method = request['path'], request['method']
        if method == 'POST' and path.endswith('ComputerSystem.Reset'):
            if request['body']['ResetType'] == 'ForceOff' and self.forceoff_works:
                self.power = 'Off'
            return {'ok': True, 'status': 204, 'body': None}
        if method == 'GET' and 'PowerState' in path:
            return {'ok': True, 'status': 200, 'body': {'PowerState': self.power}}
        if method == 'GET' and 'Boot' in path:
            return {'ok': True, 'status': 200, 'body': {'Boot': {'BootSourceOverrideEnabled': 'Disabled'}}}
        return {'ok': True, 'status': 200, 'body': {}}

    def close(self):
        self.server.shutdown()
        self.server.server_close()

    def resets(self, kind):
        return sum(1 for r in self.requests if r.get('body') and r['body'].get('ResetType') == kind)


class ReaperTest(StateTest):
    def setUp(self):
        super().setUp()
        self.controller = FakeController(m.RUN / 'bmc.sock')
        self.machine = machine.Machine({})
        self.saved_settle = machine.QUARANTINE_SETTLE_SECONDS
        machine.QUARANTINE_SETTLE_SECONDS = 0
        self.ready()

    def tearDown(self):
        machine.QUARANTINE_SETTLE_SECONDS = self.saved_settle
        self.controller.close()
        super().tearDown()

    def dead_pid(self):
        child = subprocess.Popen([sys.executable, '-c', 'pass'])
        child.wait()
        return child.pid

    def open_boot(self, boot_id, **lease):
        directory = m.boot_dir(boot_id)
        directory.mkdir(parents=True)
        m.write_json(m.STATE / m.LEASE, dict({'id': boot_id, 'heartbeat': m.now(), 'state': 'powered'}, **lease))
        return directory

    def test_a_dead_driver_is_powered_off_and_closed_137(self):
        directory = self.open_boot('20261017T000001Z-metal', pid=self.dead_pid())
        self.machine.reap()
        self.assertEqual(self.controller.power, 'Off')
        self.assertEqual((directory / 'exit-status').read_text(), '137\n')
        self.assertEqual((directory / 'ended-by').read_text(), 'reaper\n')
        self.assertIsNone(m.read_lease())

    def test_a_stale_heartbeat_is_ended_137(self):
        directory = self.open_boot('20261017T000002Z-metal', pid=os.getpid(), heartbeat=m.now() - 31)
        self.machine.reap()
        self.assertEqual((directory / 'exit-status').read_text(), '137\n')

    def test_the_attempt_ceiling_is_enforced_124(self):
        directory = self.open_boot('20261017T000003Z-metal', pid=os.getpid(), ceilingAt=m.now() - 1)
        self.machine.reap()
        self.assertEqual(self.controller.power, 'Off')
        self.assertEqual((directory / 'exit-status').read_text(), '124\n')

    def test_a_live_boot_is_left_alone(self):
        self.open_boot('20261017T000004Z-metal', pid=os.getpid(), ceilingAt=m.now() + 600)
        self.machine.reap()
        self.assertEqual(self.controller.power, 'On')
        self.assertIsNotNone(m.read_lease())

    def test_a_stray_power_on_is_powered_off(self):
        self.machine.reap()
        self.assertEqual(self.controller.power, 'Off')
        events = [json.loads(line)['event'] for line in (m.STATE / 'machine-events.jsonl').read_text().splitlines()]
        self.assertIn('stray-power-on', events)

    def test_a_stray_power_on_during_build_is_powered_off_without_releasing_the_lease(self):
        directory = self.open_boot('20261017T000008Z-metal', pid=os.getpid(), state='claimed')
        self.machine.reap()
        self.assertEqual(self.controller.power, 'Off')
        self.assertEqual(m.read_lease()['id'], directory.name)
        self.assertFalse((directory / 'ended').exists())

    def test_failed_stray_power_off_quarantines_without_relocking(self):
        directory = self.open_boot('20261017T000010Z-metal', pid=os.getpid(), state='claimed')
        with patch.object(m, 'force_off_and_wait', return_value=False):
            self.machine.reap()
        self.assertEqual(m.read_lease()['state'], 'quarantined')
        self.assertEqual(m.read_lease()['id'], directory.name)

    def test_a_failed_power_off_quarantines_until_a_retry_succeeds(self):
        self.controller.forceoff_works = False
        saved = m.POWER_OFF_SECONDS
        m.POWER_OFF_SECONDS = 1
        try:
            directory = self.open_boot('20261017T000005Z-metal', pid=self.dead_pid())
            self.machine.end_boot(m.read_lease(), 137, 'test')
        finally:
            m.POWER_OFF_SECONDS = saved
        self.assertEqual(m.read_lease()['state'], 'quarantined')
        self.assertEqual((directory / 'exit-status').read_text(), '137\n')
        with self.assertRaises(m.Quarantined):
            m.claim_lease({'id': 'next', 'pid': os.getpid()}, wait=False)
        for retry in range(1, 4):
            lease = m.read_lease()
            lease['lastRetry'] = 0
            m.write_json(m.STATE / m.LEASE, lease)
            self.machine.reap()
            self.assertEqual(m.read_lease()['retries'], retry)
        self.assertFalse(any(r['op'] == 'ipmi' and r['args'][:2] == ['chassis', 'power'] for r in self.controller.requests))
        lease = m.read_lease()
        lease['lastRetry'] = 0
        m.write_json(m.STATE / m.LEASE, lease)
        self.machine.reap()
        self.assertTrue(any(r['op'] == 'ipmi' and r['args'][:3] == ['chassis', 'power', 'off']
                            for r in self.controller.requests))
        self.assertIsNone(m.read_lease())

    def test_a_quarantine_lifts_on_the_first_retry_that_reads_off(self):
        self.open_boot('20261017T000006Z-metal', pid=os.getpid())
        m.quarantine_lease('20261017T000006Z-metal', 'injected')
        lease = m.read_lease()
        lease['lastRetry'] = 0
        m.write_json(m.STATE / m.LEASE, lease)
        self.machine.reap()
        self.assertEqual(self.controller.power, 'Off')
        self.assertIsNone(m.read_lease())

    def test_reconciliation_quarantines_failed_power_off_without_relocking(self):
        with patch.object(m, 'force_off_and_wait', return_value=False):
            machine.reconcile()
        self.assertEqual(m.read_lease()['state'], 'quarantined')
        self.assertTrue(m.machine_ready())
        with self.assertRaises(m.Quarantined):
            m.claim_lease({'id': 'next', 'pid': os.getpid()}, wait=False)

    def test_reaper_and_reconciliation_restore_inventory_after_interrupted_setup(self):
        self.open_boot('20261017T000012Z-metal', pid=self.dead_pid(), mode='setup')
        self.machine.reap()
        resets = [r for r in self.controller.requests if r.get('path') == m.LC_ATTRIBUTES]
        self.assertTrue(any(r.get('body') == {'Attributes': m.EXPECTED_LC} for r in resets))
        self.controller.requests.clear()
        machine.reconcile()
        resets = [r for r in self.controller.requests if r.get('path') == m.LC_ATTRIBUTES]
        self.assertTrue(any(r.get('body') == {'Attributes': m.EXPECTED_LC} for r in resets))

    def test_reconciliation_closes_open_records_and_powers_off_first(self):
        directory = m.boot_dir('20261017T000007Z-metal')
        directory.mkdir(parents=True)
        (m.RUN / 'ready.json').unlink()
        m.stage('20261017T000007Z-metal', '/nonexistent', 'x', 1, 'application/efi')
        machine.reconcile()
        self.assertEqual(self.controller.power, 'Off')
        self.assertEqual((directory / 'exit-status').read_text(), '137\n')
        self.assertEqual((directory / 'ended-by').read_text(), 'reconciliation\n')
        self.assertFalse((m.STATE / 'stage' / 'current.json').exists())
        self.assertTrue(m.machine_ready())
        events = [json.loads(line)['event'] for line in (m.STATE / 'machine-events.jsonl').read_text().splitlines()]
        self.assertLess(events.index('reconciliation-power-off'), events.index('reconciliation-done'))


class QueueTest(unittest.TestCase):
    def test_requests_are_spaced_and_power_off_goes_first(self):
        order = []

        class Controller:
            def redfish(self, method, path, body):
                order.append((path, time.monotonic()))
                time.sleep(0.05)
                return {'ok': True}
        saved = machine.SPACING_SECONDS
        machine.SPACING_SECONDS = 0.2
        try:
            queue = machine.Queue(Controller())
            threads = [threading.Thread(target=queue.submit, args=({'op': 'redfish', 'method': 'GET',
                                                                     'path': f'/read{i}', 'priority': 1},))
                       for i in range(3)]
            for thread in threads:
                thread.start()
                time.sleep(0.01)
            off = threading.Thread(target=queue.submit, args=({'op': 'redfish', 'method': 'POST', 'path': '/off',
                                                                'priority': 0},))
            off.start()
            for thread in threads + [off]:
                thread.join()
        finally:
            machine.SPACING_SECONDS = saved
        self.assertEqual(len(order), 4)
        self.assertEqual(order[1][0], '/off')
        gaps = [later[1] - earlier[1] for earlier, later in zip(order, order[1:])]
        self.assertTrue(all(gap >= 0.2 for gap in gaps), gaps)


class PayloadTest(unittest.TestCase):
    def test_hang_efi_is_a_pe32plus_efi_application(self):
        image = metal_image.hang_efi()
        self.assertEqual(image[:2], b'MZ')
        pe = struct.unpack_from('<I', image, 0x3c)[0]
        self.assertEqual(image[pe:pe + 4], b'PE\0\0')
        machine_type, sections, _, _, _, optional_size, characteristics = struct.unpack_from('<HHIIIHH', image, pe + 4)
        self.assertEqual((machine_type, sections, optional_size), (0x8664, 2, 240))
        self.assertFalse(characteristics & 0x0001, 'relocations must not be marked stripped')
        optional = pe + 24
        self.assertEqual(struct.unpack_from('<H', image, optional)[0], 0x20b)
        entry = struct.unpack_from('<I', image, optional + 16)[0]
        subsystem = struct.unpack_from('<H', image, optional + 68)[0]
        self.assertEqual(subsystem, 10)
        reloc_rva, reloc_size = struct.unpack_from('<II', image, optional + 112 + 5 * 8)
        self.assertEqual((reloc_rva, reloc_size), (0x2000, 12))
        name, virtual_size, rva, raw_size, raw_pointer = struct.unpack_from('<8sIIII', image, optional + 240)
        self.assertEqual((name.rstrip(b'\0'), rva, entry), (b'.text', 0x1000, 0x1000))
        code = image[raw_pointer:raw_pointer + virtual_size]
        self.assertEqual(code[:3], b'\x48\x8d\x35')
        target = 7 + struct.unpack_from('<i', code, 3)[0]
        self.assertTrue(code[target:].startswith(metal_image.HANG_LINE + b'\0'))
        self.assertIn(b'\xfa\xf4\xeb\xfc', code, 'cli; hlt; jmp back to cli')
        self.assertEqual(len(image) % 0x200, 0)

    def test_esp_offset_reads_the_gpt(self):
        with tempfile.NamedTemporaryFile() as disk:
            disk.truncate(4 << 20)
            header = bytearray(92)
            header[:8] = b'EFI PART'
            struct.pack_into('<QII', header, 72, 2, 128, 128)
            disk.seek(512)
            disk.write(header)
            entry = bytearray(128)
            entry[:16] = uuid.UUID('ebd0a0a2-b9e5-4433-87c0-68b6b72699c7').bytes_le
            disk.seek(1024)
            disk.write(entry)
            entry[:16] = metal_image.ESP_TYPE.bytes_le
            struct.pack_into('<QQ', entry, 32, 2048, 4095)
            disk.write(entry)
            disk.flush()
            self.assertEqual(metal_image.esp_offset(disk.name), (2048 * 512, 2048 * 512))


class DefinitionTest(unittest.TestCase):
    def test_final_marker_is_the_last_stage_of_the_newest_milestone_the_mode_measures(self):
        catalog = {'stages': [{'name': 'Kernel entry point', 'marker': 'Kernel entry point reached'}]}
        metal = {'milestones': [
            {'id': 'delivery', 'kernel': True, 'stages': {'x86_64': [{'name': 'Handoff', 'host': 'handoff'}]}},
            {'id': 'first-light', 'kernel': True, 'stages': {'x86_64': ['Kernel entry point']}},
            {'id': 'shell', 'stages': {'shell': [{'name': 'Proof', 'marker': 'metal-console-ok 42'}]}}]}
        self.assertEqual(boot.final_marker(metal, 'default', catalog), ('Kernel entry point reached', 'contains'))
        self.assertEqual(boot.final_marker(metal, 'shell', catalog), ('metal-console-ok 42', 'contains'))
        self.assertIsNone(boot.final_marker({'milestones': [metal['milestones'][0]]}, 'default', catalog))

    def test_boot_progress_key(self):
        self.assertEqual(boot.progress_key(None), (None, '', None))
        self.assertEqual(boot.progress_key({'LastState': 'OEM', 'OemLastState': 'No bootable devices.'}),
                         ('OEM', 'No bootable devices.', None))


class DriverTest(StateTest):
    def setUp(self):
        super().setUp()
        self.args = SimpleNamespace(id='20261017T000009Z-metal', mode='default', payload='image',
                                    inject=None, serve_as=None, content_type=None, no_wait=False)
        with patch.object(m, 'load_site', return_value={}):
            self.driver = boot.Driver(self.args)
        self.driver.dir.mkdir(parents=True)
        self.driver.event = Mock()
        self.driver.phase = Mock()
        self.driver.t0 = 100

    def finish(self, call, status):
        with self.assertRaises(boot.Finish) as raised:
            call()
        self.assertEqual(raised.exception.status, status)
        return raised.exception

    def test_deadlines_require_the_actual_phase_evidence(self):
        d = self.driver
        d.post_budget = 20
        with patch.object(m, 'now', return_value=121):
            self.finish(d.check_deadlines, m.EXIT_HUNG)
            d.request_at = 110
            d.check_deadlines()
        with patch.object(m, 'now', return_value=171):
            self.finish(d.check_deadlines, m.EXIT_DELIVERY)
        d.first_byte_at = 111
        d.last_progress = 112
        with patch.object(m, 'now', return_value=128):
            self.finish(d.check_deadlines, m.EXIT_DELIVERY)
        d.last_progress = 171
        with patch.object(m, 'now', return_value=172):
            self.finish(d.check_deadlines, m.EXIT_DELIVERY)
        d.complete_at = 120
        with patch.object(m, 'now', return_value=181):
            self.finish(d.check_deadlines, m.EXIT_DELIVERY)
        d.seen.add('bootloader')
        with patch.object(m, 'now', return_value=241):
            self.assertTrue(self.finish(d.check_deadlines, m.EXIT_HUNG).hung)
        d.kernel_started = 125
        d.last_serial = 425
        with patch.object(m, 'now', return_value=426):
            self.finish(d.check_deadlines, m.EXIT_SHORT)
        d.lease_ceiling = 400
        with patch.object(m, 'now', return_value=426):
            self.finish(d.check_deadlines, m.EXIT_HUNG)

    def test_404_and_fatal_capture_graces(self):
        d = self.driver
        d.request_at = d.not_found_at = 110
        with patch.object(m, 'now', return_value=139):
            d.check_deadlines()
        with patch.object(m, 'now', return_value=140):
            self.finish(d.check_deadlines, m.EXIT_DELIVERY)
        d.fatal_at = 130
        with patch.object(m, 'now', return_value=135):
            self.finish(d.check_deadlines, m.EXIT_SHORT)

    def test_http_requires_a_matching_complete_transfer(self):
        d = self.driver
        d.on_http({'event': 'head', 't': 99, 'id': d.id})
        self.assertIsNone(d.request_at)
        d.on_http({'event': '404', 't': 101, 'route': False})
        self.assertIsNone(d.request_at)
        with patch.object(m, 'record_post_time') as record:
            d.on_http({'event': 'head', 't': 110, 'id': d.id})
            record.assert_called_once_with(d.id, 10)
        self.assertIsNone(d.complete_at)
        d.on_http({'event': 'complete', 't': 111, 'id': 'another', 'matches': True})
        self.assertIsNone(d.complete_at)
        d.on_http({'event': 'start', 't': 112, 'id': d.id})
        d.on_http({'event': 'progress', 't': 113, 'id': d.id, 'bytes': 1, 'total': 1})
        self.finish(lambda: d.on_http({'event': 'complete', 't': 114, 'id': d.id, 'matches': False}),
                    m.EXIT_DELIVERY)
        self.assertIsNone(d.complete_at)
        d.on_http({'event': 'complete', 't': 115, 'id': d.id, 'matches': True, 'sha256': 'digest'})
        self.assertEqual(d.complete_at, 115)
        self.assertEqual(d.delivery['transferSeconds'], 3)
        d.kernel_started = 116
        self.finish(lambda: d.on_http({'event': '404', 't': 117, 'route': True}), m.EXIT_SHORT)

    def test_reset_detection_uses_serial_and_boot_progress(self):
        d = self.driver
        d.on_line(boot.BOOTLOADER_LINE)
        d.on_line(boot.HANDOFF_LINE)
        self.assertIsNotNone(d.kernel_started)
        self.finish(lambda: d.on_line(boot.BOOTLOADER_LINE), m.EXIT_SHORT)
        for state in boot.RUNNING_STATES:
            d.bmc_events = [{'progress': {'LastState': state}}]
            d.pump_bmc()
        d.bmc_events = [{'progress': {'LastState': 'MemoryInit'}}]
        self.finish(d.pump_bmc, m.EXIT_SHORT)

    def test_finish_does_not_discard_the_rest_of_a_serial_read(self):
        d = self.driver
        d.final_marker = ('DONE|FINISHED', 'line')
        (d.dir / 'serial.raw').write_bytes(b'prefix\nDONE\nbacktrace\npartial')
        self.finish(d.pump_serial, m.EXIT_DONE)
        self.assertEqual((d.dir / 'serial.txt').read_text(), 'prefix\nDONE\nbacktrace\npartial')
        self.assertEqual(d.line, 'partial')
        with (d.dir / 'serial.raw').open('ab') as raw:
            raw.write(b' end\n')
        d.pump_serial()
        self.assertEqual(d.line, '')
        self.assertTrue((d.dir / 'serial.txt').read_text().endswith('partial end\n'))

    def test_final_marker_alternatives_and_exact_lines(self):
        self.assertTrue(boot.marker_matches(('DONE|FINISHED', 'line'), 'FINISHED'))
        self.assertFalse(boot.marker_matches(('DONE|FINISHED', 'line'), 'not DONE yet'))
        self.assertTrue(boot.marker_matches(('DONE|FINISHED', 'contains'), 'stage FINISHED'))
        definition = {'milestones': [{'stages': {'default': [{'marker': 'DONE|OK', 'match': 'line'}]}}]}
        self.assertEqual(boot.final_marker(definition, 'default', {}), ('DONE|OK', 'line'))

    def test_failed_preflight_power_off_quarantines(self):
        self.ready()
        m.claim_lease({'id': self.driver.id, 'pid': os.getpid()}, wait=False)
        with patch.object(m, 'machine_checks', return_value=([], {'power': 'On'})), \
             patch.object(m, 'force_off_and_wait', return_value=False):
            self.finish(self.driver.preflight, m.EXIT_DELIVERY)
        self.assertEqual(m.read_lease()['state'], 'quarantined')
        self.assertFalse(m.release_lease(self.driver.id))

    def test_uncertain_power_on_keeps_quarantine_after_power_off(self):
        d = self.driver
        self.ready()
        m.claim_lease({'id': d.id, 'pid': os.getpid()}, wait=False)
        def rf(method, *args, **kwargs):
            if method == 'POST':
                self.assertEqual(m.read_lease()['state'], 'powered')
                raise m.BmcError('queue timeout')
            return {}
        with patch.object(m, 'rf', side_effect=rf), \
             patch.object(m, 'rf_get', return_value={'Boot': {'BootSourceOverrideEnabled': 'Once',
                                                           'BootSourceOverrideTarget': 'UefiHttp'}}):
            self.finish(d.power_on, m.EXIT_DELIVERY)
        self.assertIsNotNone(d.t0)
        with patch.object(m, 'force_off_and_wait', return_value=True):
            self.assertTrue(d.power_off(False))
        self.assertEqual(m.read_lease()['state'], 'quarantined')
        self.assertFalse(m.release_lease(d.id))

    def test_queued_boot_can_stop_before_or_after_waiting(self):
        self.ready()
        m.claim_lease({'id': 'other', 'pid': os.getpid()}, wait=False)
        d = self.driver
        d.t0 = None
        def stop(delay):
            (d.dir / 'stop-request').touch()
        with patch.object(m.time, 'sleep', side_effect=stop):
            self.finish(d.claim, m.EXIT_CANCELLED)
        self.assertEqual(m.read_lease()['id'], 'other')
        m.release_lease('other')
        self.finish(d.claim, m.EXIT_CANCELLED)
        self.assertIsNone(m.read_lease())

    def test_preflight_checks_ahci_and_inventory_collection(self):
        site = m.load_site(FIXTURES / 'redact-site.env')
        self.assertEqual(m.expected_bios(site)['EmbSata'], 'AhciMode')
        def get(path):
            if path == m.LC_ATTRIBUTES:
                return {'Attributes': {'LCAttributes.1.CollectSystemInventoryOnRestart': 'Enabled'}}
            if path == m.SYSTEM + '/Bios':
                return {'Attributes': dict(m.expected_bios(site), **m.KEPT_BIOS)}
            if path == m.MANAGER_ATTRIBUTES:
                return {'Attributes': m.EXPECTED_MANAGER}
            return {'PowerState': 'Off', 'Boot': {'BootSourceOverrideEnabled': 'Disabled'}}
        with patch.object(m, 'rf_get', side_effect=get):
            rows, _ = m.machine_checks(site)
        self.assertEqual([row[0] for row in m.failed_checks(rows)],
                         ['lc.LCAttributes.1.CollectSystemInventoryOnRestart'])

    def test_follow_refuses_shell_input_before_remote_execution(self):
        run = load_script('metal-run')
        with patch.object(run, 'in_metal') as remote:
            with self.assertRaises(SystemExit):
                run.follow({}, 'id; touch injected')
            remote.assert_not_called()

    def test_sha_requires_target_ancestry(self):
        run = load_script('metal-run')
        with patch.object(run, 'resolve', return_value='b' * 40), \
             patch.object(run, 'in_build', return_value=SimpleNamespace(returncode=1)) as remote:
            with self.assertRaises(SystemExit):
                run.verify_target({'BUILD_REPO': 'fixture'}, 'a' * 40, 'main')
            self.assertIn('merge-base', remote.call_args.args)


class BuildPolicyTest(unittest.TestCase):
    def test_guard_comes_from_origin_main_instead_of_the_target_tree(self):
        build = load_script('metal-build')
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            repo, remote = root / 'repo', root / 'origin.git'
            def git(*args):
                return subprocess.run(['git', *map(str, args)], check=True, capture_output=True, text=True).stdout.strip()
            git('init', '--bare', remote)
            git('init', '-b', 'main', repo)
            git('-C', repo, 'config', 'user.name', 'Fixture')
            git('-C', repo, 'config', 'user.email', 'fixture@example.invalid')
            guard = repo / 'scripts/effort-target-guard.py'
            guard.parent.mkdir()
            guard.write_text("import sys\nTARGETS = ('x86-metal',)\nsys.exit(3)\n")
            git('-C', repo, 'add', '.')
            git('-C', repo, 'commit', '-m', 'Set the policy')
            git('-C', repo, 'remote', 'add', 'origin', remote)
            git('-C', repo, 'push', 'origin', 'main')
            guard.write_text("import sys\nTARGETS = ('x86-metal',)\nsys.exit(0)\n")
            git('-C', repo, 'commit', '-am', 'Attempt to bypass the policy')
            sha = git('-C', repo, 'rev-parse', 'HEAD')
            args = SimpleNamespace(id='20261017T000011Z-metal', root=str(root), repo=str(repo), sha=sha,
                                   rust_fork=str(root / 'rust'), cache=str(root / 'cache'))
            self.assertEqual(build.outer(args), 3)


class PublicRepositoryTest(unittest.TestCase):
    def test_tools_carry_no_site_values(self):
        """Detect numeric IPv4, colon-form MAC and common home-path literals; names require human review."""
        paths = [p for p in TOOLS.rglob('*') if p.is_file() and 'fixtures' not in p.parts and '__pycache__' not in p.parts]
        paths.append(TOOLS.parents[1] / 'docs' / 'metal' / 'README.md')
        for path in paths:
            text = path.read_text(errors='replace')
            for match in re.finditer(r'\b\d{1,3}(?:\.\d{1,3}){3}\b', text):
                self.assertIn(match.group(0), ('127.0.0.1', '0.0.0.0', '255.255.255.0'), f'{path.name}: {match.group(0)}')
            self.assertIsNone(re.search(r'([0-9A-Fa-f]{2}:){5}[0-9A-Fa-f]{2}', text), path.name)
            self.assertIsNone(re.search(r'/Users/\w|/home/\w', text), path.name)


if __name__ == '__main__':
    unittest.main()
