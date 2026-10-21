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
import shutil
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
                if len(value) >= 4 or m.SECRET_KEYS.search(key) or value in m.identifier_list(self.site):
                    self.assertEqual(self.redact(value), '<site>')
                else:
                    self.assertEqual(self.redact(value), value)
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

    def test_digits_and_commit_ids_survive(self):
        for text in ('0123456789', '0 1 2 3 4 5 6 7 8 9', 'commit 14a0ff0c', 'sha256 ' + '3a7b' * 16,
                     '{"t": 1792203213.123, "seconds": 3.2}', 'ratio 3:2', 'v1.2.3'):
            self.assertEqual(self.redact(text), text)

    def test_private_values_are_whole_tokens(self):
        for value in (self.site['IDRAC_PASSWORD'], 'fixturehost', 'FX12345', 'fixtureauto'):
            self.assertEqual(self.redact('value="' + value + '"'), 'value="<site>"')
            self.assertEqual(self.redact('prefix' + value + 'suffix'), 'prefix' + value + 'suffix')
            self.assertEqual(self.redact('value=' + value + '.'), 'value=<site>.')
            self.assertEqual(self.redact('value:' + value), 'value:<site>')

    def test_short_explicit_identifiers_and_credentials_still_mask(self):
        redact = m.Redactor({'IDRAC_USER': 'u', 'IDRAC_PASSWORD': 'p', 'REDACT_IDENTIFIERS': 'x'})
        self.assertEqual(redact('user=u password=p host=x'), 'user=<site> password=<site> host=<site>')

    def test_digits_and_commit_ids_survive_site_configuration(self):
        text = ('3 13 33 1234 82403 12:34:56 1.2.3 ratio 3:2 seconds=3 '
                'commit=3abc8240def12345 0193b1d86dd372ec891bae799f6da20deef16fc1')
        self.assertEqual(self.redact(text), text)
        event = {'event': 'power-on', 'seconds': 3, 'commit': '3abc8240def12345'}
        self.assertEqual(json.loads(self.redact(json.dumps(event))), event)

    def test_identifiers_match_whole_tokens(self):
        for name in ('fixturehost', 'fixture-container', 'FX12345', 'fixtureauto',
                     self.site['IDRAC_PASSWORD']):
            self.assertEqual(self.redact('name="' + name + '"'), 'name="<site>"')
            self.assertEqual(self.redact('prefix' + name + 'suffix'), 'prefix' + name + 'suffix')
        self.assertEqual(self.redact('http://fixturehost:8240/'), 'http://<site>:<site>/')

    def test_url_path_components_are_private(self):
        redact = m.Redactor({'BOOT_URL': 'http://fixture.invalid/private-folder/payload.efi'})
        for text in ('http://fixture.invalid/private-folder/payload.efi', 'fixture.invalid',
                     '/private-folder/payload.efi', 'private-folder', 'payload.efi'):
            self.assertEqual(redact(text), '<site>')

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

    def test_probe_file_does_not_consume_firmware_transfer(self):
        from metal_probe import PROBE_PATH, PROBE_SHA256
        m.stage('20261017T000000Z-metal', self.image, self.digest, self.image.stat().st_size, 'application/efi')
        url = f'http://127.0.0.1:{self.port}' + PROBE_PATH
        for _ in range(2):
            status, _, body = self.request(url=url)
            self.assertEqual(status, 200)
            self.assertEqual(len(body), 128 * 1024)
            self.assertEqual(hashlib.sha256(body).hexdigest(), PROBE_SHA256)
        self.assertFalse(m.read_json(m.STATE / 'stage/current.json').get('served'))
        self.assertEqual(self.request()[0], 200)
        self.wait_complete()
        self.assertEqual(self.request()[0], 404)

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

    def test_a_stray_power_on_is_powered_off_after_its_grace(self):
        # An operator's power-on becomes a persistent session at the boot manager's question; until the grace has
        # passed, a power-on without a lease is left alone.
        self.machine.reap()
        self.assertEqual(self.controller.power, 'On')
        self.machine.last_power_check = 0
        self.machine.stray_since -= machine.STRAY_GRACE_SECONDS
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
        entered = threading.Event()
        release = threading.Event()

        class Controller:
            def redfish(self, method, path, body):
                order.append((path, time.monotonic()))
                if path == '/read0':
                    entered.set()
                    if not release.wait(10):
                        raise RuntimeError('test request was not released')
                time.sleep(0.05)
                return {'ok': True}
        saved = machine.SPACING_SECONDS
        machine.SPACING_SECONDS = 0.2
        try:
            queue = machine.Queue(Controller())
            threads = [threading.Thread(target=queue.submit, args=({'op': 'redfish', 'method': 'GET',
                                                                     'path': f'/read{i}', 'priority': 1},))
                       for i in range(3)]
            threads[0].start()
            self.assertTrue(entered.wait(10))
            for thread in threads[1:]:
                thread.start()
            off = threading.Thread(target=queue.submit, args=({'op': 'redfish', 'method': 'POST', 'path': '/off',
                                                                'priority': 0},))
            off.start()
            with queue.condition:
                self.assertTrue(queue.condition.wait_for(lambda: len(queue.heap) == 3, timeout=10))
            release.set()
            for thread in threads + [off]:
                thread.join()
        finally:
            release.set()
            machine.SPACING_SECONDS = saved
        self.assertEqual(len(order), 4)
        self.assertEqual(order[1][0], '/off')
        gaps = [later[1] - earlier[1] for earlier, later in zip(order, order[1:])]
        self.assertTrue(all(gap >= 0.2 for gap in gaps), gaps)

    def test_power_off_overtakes_read_during_pacing(self):
        order = []
        controller = Mock()
        controller.redfish.side_effect = lambda method, path, body: (order.append(path) or {'ok': True})
        queue = machine.Queue(controller)
        queue.last_end = time.monotonic() + 1
        read = threading.Thread(target=queue.submit, args=({'op': 'redfish', 'method': 'GET',
                                                          'path': '/read', 'priority': 1},))
        read.start()
        # The worker must leave the read in the queue until dispatch, even while pacing.
        with queue.condition:
            self.assertTrue(queue.condition.wait_for(lambda: len(queue.heap) == 1, timeout=10))
        off = threading.Thread(target=queue.submit, args=({'op': 'redfish', 'method': 'POST',
                                                         'path': '/off', 'priority': 0},))
        off.start()
        read.join(15)
        off.join(15)
        self.assertEqual(order, ['/off', '/read'])


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

    def stub(self, directory, code=b'\x90\xc3' + metal_image.STUB_MAGIC + b'\0' * 16):
        path = Path(directory) / 'stub.efi'
        path.write_bytes(metal_image.pe32plus(code))
        return path

    def test_stub_efi_appends_the_disk_as_a_section_and_fills_in_the_record(self):
        with tempfile.TemporaryDirectory() as directory:
            stub = self.stub(directory)
            disk = Path(directory) / 'disk.img'
            content = bytes(range(256)) * 41 + b'end'
            disk.write_bytes(content)
            facts = metal_image.stub_efi(stub, disk, Path(directory) / 'boot.efi')
            image = (Path(directory) / 'boot.efi').read_bytes()
        self.assertEqual((facts['size'], facts['disk']['size']), (len(image), len(content)))
        self.assertEqual(facts['sha256'], hashlib.sha256(image).hexdigest())
        pe = struct.unpack_from('<I', image, 0x3c)[0]
        sections, optional_size = struct.unpack_from('<H', image, pe + 6)[0], struct.unpack_from('<H', image, pe + 20)[0]
        optional = pe + 24
        section_align, file_align = struct.unpack_from('<II', image, optional + 32)
        table = [struct.unpack_from(metal_image.SECTION_HEADER, image, optional + optional_size + 40 * index)
                 for index in range(sections)]
        self.assertEqual([section[0].rstrip(b'\0') for section in table], [b'.text', b'.reloc', b'.bimg'])
        name, virtual_size, rva, raw_size, raw, *_, characteristics = table[2]
        self.assertEqual((virtual_size, characteristics), (len(content), 0x40000040))
        self.assertEqual((rva % section_align, raw % file_align, raw_size % file_align), (0, 0, 0))
        self.assertGreaterEqual(rva, table[1][2] + table[1][1])
        self.assertEqual(struct.unpack_from('<I', image, optional + 56)[0], rva + metal_image.align(len(content), section_align))
        self.assertEqual(image[raw:raw + len(content)], content)
        self.assertEqual(len(image), raw + raw_size)
        record = image.find(metal_image.STUB_MAGIC)
        text = table[0]
        record_rva = text[2] + record - text[4]
        offset, size = struct.unpack_from('<qQ', image, record + len(metal_image.STUB_MAGIC))
        self.assertEqual((record_rva + offset, size), (rva, len(content)))

    def test_stub_efi_refuses_a_stub_without_its_record_or_already_wrapped(self):
        with tempfile.TemporaryDirectory() as directory:
            disk = Path(directory) / 'disk.img'
            disk.write_bytes(b'\0' * 1024)
            with self.assertRaisesRegex(ValueError, 'payload record'):
                metal_image.stub_efi(self.stub(directory, b'\x90\xc3'), disk, Path(directory) / 'out.efi')
            metal_image.stub_efi(self.stub(directory), disk, Path(directory) / 'once.efi')
            with self.assertRaisesRegex(ValueError, 'already carries'):
                metal_image.stub_efi(Path(directory) / 'once.efi', disk, Path(directory) / 'twice.efi')

    def test_the_stub_source_carries_the_record_magic_the_image_step_looks_for(self):
        source = (TOOLS / 'efi-stub/src/main.rs').read_text()
        literal = re.search(r'magic: \*b"([^"]*)"', source).group(1).replace('\\0', '\0').encode()
        self.assertEqual(literal, metal_image.STUB_MAGIC)

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
    def test_final_marker_skips_stub_milestones(self):
        catalog = {'stages': [{'name': 'Kernel entry point', 'marker': 'Kernel entry point reached'}]}
        metal = {'milestones': [
            {'id': 'first-light', 'bootShapes': ['ramdisk'], 'kernel': True, 'stages': {'x86_64': [
                'Kernel entry point', {'name': 'Kernel idle', 'marker': '[metal] kernel idle'}]}},
            {'id': 'smp', 'bootShapes': ['ramdisk'], 'kernel': True, 'stub': True, 'stages': {'x86_64': [
                {'name': 'Four CPUs', 'marker': '[smp] online=4 reported=4'}]}},
            {'id': 'first-userspace', 'bootShapes': ['ramdisk'], 'stub': True, 'stages': {'default': [
                {'name': 'Boot script completed', 'marker': '[init] Boot script completed'}]}}]}
        self.assertEqual(boot.final_marker(metal, 'default', catalog), ('[metal] kernel idle', 'contains'))
        metal['milestones'][2].pop('stub')
        self.assertEqual(boot.final_marker(metal, 'default', catalog), ('[init] Boot script completed', 'contains'))

    def test_final_marker_is_the_last_stage_of_the_newest_milestone_the_mode_measures(self):
        catalog = {'stages': [{'name': 'Kernel entry point', 'marker': 'Kernel entry point reached'}]}
        metal = {'milestones': [
            {'id': 'delivery', 'bootShapes': ['ramdisk'], 'kernel': True, 'stages': {'x86_64': [{'name': 'Handoff', 'host': 'handoff'}]}},
            {'id': 'first-light', 'bootShapes': ['ramdisk'], 'kernel': True, 'stages': {'x86_64': ['Kernel entry point']}},
            {'id': 'shell', 'bootShapes': ['ramdisk'], 'stages': {'shell': [{'name': 'Proof', 'marker': 'metal-console-ok 42'}]}}]}
        self.assertEqual(boot.final_marker(metal, 'default', catalog), ('Kernel entry point reached', 'contains'))
        self.assertEqual(boot.final_marker(metal, 'shell', catalog), ('metal-console-ok 42', 'contains'))
        self.assertIsNone(boot.final_marker({'milestones': [metal['milestones'][0]]}, 'default', catalog))

    def test_explicit_mode_terminal_precedes_later_kernel_milestones(self):
        definition = {'milestones': [
            {'bootShapes': ['ramdisk'], 'stages': {'default': [{'marker': 'INIT DONE|INIT COMPLETE', 'match': 'line'}]}},
            {'kernel': True, 'bootShapes': ['ramdisk'], 'stages': {'x86_64': [{'marker': 'NETWORK READY'}]}}]}
        self.assertEqual(boot.final_marker(definition, 'default', {}),
                         ('INIT DONE|INIT COMPLETE', 'line'))

    def test_boot_progress_key(self):
        self.assertEqual(boot.progress_key(None), (None, '', None))
        self.assertEqual(boot.progress_key({'LastState': 'OEM', 'OemLastState': 'No bootable devices.'}),
                         ('OEM', 'No bootable devices.', None))


class StagingPrerequisiteTest(unittest.TestCase):
    def test_missing_tool_reports_install_command_before_staging(self):
        import metal_root
        with patch.object(metal_root.shutil, 'which', return_value=None):
            with self.assertRaisesRegex(ValueError, 'missing prerequisite: debugfs .*install with: .*e2fsprogs'):
                metal_root.require_e2fsprogs('debugfs')

    def test_distribution_sbin_is_searched_when_it_is_not_on_path(self):
        import metal_root
        with patch.object(metal_root.shutil, 'which', side_effect=lambda p: p if p == '/usr/sbin/debugfs' else None):
            self.assertEqual(metal_root.require_e2fsprogs('debugfs'), ['/usr/sbin/debugfs'])


class MissingStagingToolCleanupTest(StateTest):
    def test_missing_debugfs_cancels_claimed_boot_and_cleans_resources(self):
        import metal_root
        args = SimpleNamespace(id='missing-tool-fixture', payload='image', mode='default',
                               target='main', task=None, commit=None, inject=None)
        with patch.object(m, 'load_site', return_value={}):
            driver = boot.Driver(args)

        def claim():
            driver.claimed = True
            driver.stage.mkdir(parents=True)

        with patch.object(driver, 'claim', side_effect=claim), \
                patch.object(driver, 'prepare_payload', side_effect=lambda: metal_root.stage_root(
                    driver.stage / 'root.img', 'default', None, {})), \
                patch.object(driver, 'pump_serial'), \
                patch.object(metal_root.shutil, 'which', return_value=None), \
                patch.object(m, 'clear_override') as clear, \
                patch.object(m, 'unstage') as unstage, \
                patch.object(m, 'sol_deactivate') as sol, \
                patch.object(m, 'release_lease') as release, \
                patch.object(boot.subprocess, 'run') as command:
            self.assertEqual(driver.run(), m.EXIT_CANCELLED)
        self.assertFalse(driver.stage.exists())
        clear.assert_called_once_with()
        unstage.assert_called_once_with(driver.id)
        sol.assert_called_once_with()
        release.assert_called_once_with(driver.id)
        command.assert_called_once_with(driver.tmux + ['kill-server'], capture_output=True)
        events = [json.loads(line) for line in (driver.dir / 'events.jsonl').read_text().splitlines()]
        self.assertTrue(any(event.get('event') == 'driver-error'
                            for event in events))
        self.assertEqual(int((driver.dir / 'exit-status').read_text()), m.EXIT_CANCELLED)


class RootStagingTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        import metal_root
        cls.mke2fs, cls.debugfs = metal_root.require_e2fsprogs('mke2fs', 'debugfs')

    def test_modes_and_guest_site_are_read_back_from_the_staged_image(self):
        import metal_root
        mke2fs, debugfs = self.mke2fs, self.debugfs
        with tempfile.TemporaryDirectory() as scratch:
            root = Path(scratch)
            tree = root / 'tree'
            (tree / 'sbin').mkdir(parents=True)
            (tree / 'etc/breenix').mkdir(parents=True)
            (tree / 'bin').mkdir()
            for program in ('getty', 'login'):
                (tree / 'bin' / program).write_bytes(b'\x7fELFfixture')
            (tree / 'sbin/probe').write_bytes(b'\x7fELFprobe')
            (tree / 'sbin/init').write_bytes(b'\x7fELFfixture')
            (tree / 'sbin/suite-smoke').write_bytes(b'\x7fELFsuite')
            (tree / 'sbin/suite-time').write_bytes(b'\x7fELFsuite')
            image = root / 'root.img'
            subprocess.run([mke2fs, '-q', '-t', 'ext2', '-d', str(tree), '-F', str(image), '4096'],
                           check=True, capture_output=True)
            site = {'GUEST_FETCH_URL': 'fixture-value', 'IDRAC_PASSWORD': 'private-fixture',
                    'CONSOLE_USER': 'root', 'CONSOLE_PASSWORD': 'fixture_password_only_12345',
                    'CONSOLE_IDENTITY_KEY': 'ab' * 32}
            helper = os.environ.get('CONSOLE_HASH_HELPER', str(Path(__file__).resolve().parents[1] / 'target/host-test-tools/sha512-crypt'))
            prep_message = ('Missing or stale console hash helper: run scripts/build-host-test-tools.sh '
                            '(CONSOLE_HASH_HELPER may select an existing executable)')
            self.assertTrue(helper and Path(helper).is_file() and os.access(helper, os.X_OK),
                            prep_message)
            hasher = Path(helper)
            if 'CONSOLE_HASH_HELPER' not in os.environ:
                source = Path(__file__).resolve().parents[1] / 'scripts/metal/sha512-crypt.rs'
                self.assertGreaterEqual(hasher.stat().st_mtime_ns, source.stat().st_mtime_ns,
                                        prep_message)
            # A tests boot's root is a default boot's: the testing kernel does not read the target.
            for mode, suite, expected in [('default', None, 'default'), ('shell', None, 'shell'),
                                          ('suite', 'smoke', 'suite smoke hostkeys xhci-replay'), ('tests', None, 'default')]:
                facts = metal_root.stage_root(image, mode, suite, site, hasher=hasher)
                self.assertEqual(facts['siteFields'], 1)
                def read(name):
                    return subprocess.run([debugfs, '-R', 'cat /etc/breenix/' + name, str(image)],
                                          check=True, capture_output=True).stdout.decode()
                self.assertEqual(read('boot-target'), expected + '\n')
                self.assertEqual(read('site.env'), 'FETCH_URL=fixture-value\n')
                if mode == 'default':
                    self.assertTrue(read('../shadow').startswith('root:$6$'))
            subprocess.run(['bash', str(TOOLS.parent / 'write-boot-target.sh'), str(image), '--desktop'],
                           check=True, capture_output=True)
            self.assertEqual(read('boot-target'), 'desktop\n')
            probe_site = dict(site, BOOT_URL='http://fixture.invalid/breenix/boot.img')
            metal_root.stage_root(image, 'probe', None, probe_site)
            self.assertEqual(read('boot-target'), 'probe\n')
            self.assertIn('METAL_FETCH_URL=http://fixture.invalid/breenix/probe.bin\n', read('site.env'))
            config = subprocess.run([debugfs, '-R', 'stat /etc/resolv.conf', str(image)],
                                    check=True, capture_output=True).stdout.decode()
            self.assertIn('/proc/net/pnp', config)
            # Exercise the deployed layout, outside the repository's scripts directory.
            package = root / 'deployed'
            package.mkdir()
            shutil.copyfile(TOOLS / 'metal_root.py', package / 'metal_root.py')
            shutil.copyfile(TOOLS.parent / 'write-boot-target.sh', package / 'write-boot-target.sh')
            spec = importlib.util.spec_from_file_location('deployed_root', package / 'metal_root.py')
            deployed = importlib.util.module_from_spec(spec)
            spec.loader.exec_module(deployed)
            facts = deployed.stage_root(image, 'default', None, site, hasher=hasher)
            self.assertEqual(read('boot-target'), 'default\n')
            self.assertEqual(facts['writerSha256'], hashlib.sha256(
                (TOOLS.parent / 'write-boot-target.sh').read_bytes()).hexdigest())
            with self.assertRaises(ValueError):
                metal_root.stage_root(image, 'suite', 'absent', site)
            with self.assertRaises(ValueError):
                metal_root.stage_root(image, 'default', None, {'GUEST_PASSWORD': 'fixture'})
            # Several suites: the first is the boot target, the list is beside it, each binary is on the root.
            metal_root.stage_root(image, 'suite', 'smoke,time', site)
            self.assertEqual(read('boot-target'), 'suite smoke hostkeys xhci-replay\n')
            self.assertEqual(read('suite-sequence'), 'smoke,time\n')
            with self.assertRaises(ValueError):
                metal_root.stage_root(image, 'suite', 'smoke,absent', site)
            metal_root.stage_root(image, 'suite', 'time', site)
            self.assertEqual(read('boot-target'), 'suite time hostkeys xhci-replay\n')
            self.assertEqual(read('suite-sequence'), '')
            # install-persistent's userland carries the operator's account; ordinary staging removes it.
            import base64
            blob = struct.pack('>I', 11) + b'ssh-ed25519' + struct.pack('>I', 32) + bytes(range(32))
            line = 'ssh-ed25519 ' + base64.b64encode(blob).decode() + ' operator@boot-host'
            account = dict(site, OPERATOR_USER='operator', OPERATOR_SSH_PUBLIC_KEY=line)
            operator = metal_root.operator_account(account)
            self.assertEqual(operator, 'user operator\nkey ' + line + '\n')
            facts = metal_root.stage_root(image, 'default', None, site, hasher=hasher, operator=operator)
            self.assertEqual(read('operator-account'), operator)
            self.assertEqual(facts['operatorAccountSha256'], hashlib.sha256(operator.encode()).hexdigest())
            stat = subprocess.run([debugfs, '-R', 'stat /etc/breenix/operator-account', str(image)],
                                  check=True, capture_output=True).stdout.decode()
            self.assertIn('Mode:  0600', stat)
            metal_root.stage_root(image, 'default', None, site, hasher=hasher)
            self.assertEqual(read('operator-account'), '')
            for bad in ['', '-----BEGIN OPENSSH PRIVATE KEY-----', 'command="true" ' + line,
                        line.replace('ssh-ed25519 ', 'ssh-rsa ', 1), 'ssh-ed25519 !!notbase64']:
                with self.assertRaises(ValueError):
                    metal_root.operator_account(dict(account, OPERATOR_SSH_PUBLIC_KEY=bad))
            for bad in ['', 'root', 'Operator', 'a:b', 'x' * 33]:
                with self.assertRaises(ValueError):
                    metal_root.operator_account(dict(account, OPERATOR_USER=bad))
            with self.assertRaises(ValueError):
                metal_root.operator_account(site)

    def test_install_boot_stages_probe_and_the_install_record_and_names_what_will_be_written(self):
        import metal_root
        mke2fs, debugfs = self.mke2fs, self.debugfs
        with tempfile.TemporaryDirectory() as scratch:
            root = Path(scratch)
            tree = root / 'tree'
            (tree / 'sbin').mkdir(parents=True)
            (tree / 'etc/breenix').mkdir(parents=True)
            (tree / 'sbin/probe').write_bytes(b'\x7fELFprobe')
            image = root / 'root.img'
            subprocess.run([mke2fs, '-q', '-t', 'ext2', '-d', str(tree), '-F', str(image), '4096'],
                           check=True, capture_output=True)
            site = {'BOOT_URL': 'http://fixture.invalid/breenix/boot.img'}
            facts = metal_root.stage_root(image, 'probe', None, site, install='20261017T000013Z-metal')
            data = image.read_bytes()
            record = b'BREENIX-STORAGE 1\ninstall-root 20261017T000013Z-metal\n'
            self.assertEqual(data[:512], record + bytes(512 - len(record)))
            write = facts['diskWrite']
            self.assertEqual((write['installedBy'], write['lba'], write['bytes']),
                             ('20261017T000013Z-metal', 0, len(data)))
            self.assertEqual(write['sha256AfterRecord'], hashlib.sha256(data[512:]).hexdigest())
            root_record = metal_root.root_record('20261017T000013Z-metal', len(data), write['sha256AfterRecord'])
            self.assertEqual(write['rootRecordSha256'], hashlib.sha256(root_record).hexdigest())
            self.assertEqual(root_record[:18], b'BREENIX-STORAGE 1\n')
            # The filesystem is untouched by the record, and a second install request is refused.
            target = subprocess.run([debugfs, '-R', 'cat /etc/breenix/boot-target', str(image)],
                                    check=True, capture_output=True).stdout
            self.assertEqual(target, b'probe\n')
            with self.assertRaises(ValueError):
                metal_root.write_install_record(image, '20261017T000014Z-metal')
            with self.assertRaises(ValueError):
                metal_root.install_record('bad/id')


class MilestoneScoreTest(StateTest):
    def test_scores_only_measurable_shapes_and_still_rejects_missing_stages(self):
        import metal_score
        root = TOOLS.parents[1]
        directory = m.STATE / 'boot'
        directory.mkdir()
        for name in ('docs/metal/boot-path.json',
                     'tools/breenix-runs/Sources/BreenixRuns/Resources/boot-stages-x86_64.json'):
            dest = directory / 'definition' / name
            dest.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(root / name, dest)
        (directory / 'serial.txt').write_text('')
        (directory / 'events.jsonl').write_text('')
        for disk, measured, excluded in [
            ('install', 'disk-io', 'disk-filesystem'),
            ('root', 'disk-filesystem', 'disk-io'),
        ]:
            m.write_json(directory / 'run.json', {'disk': disk, 'mode': 'probe'})
            verdict = metal_score.score(directory)
            self.assertIn(measured, verdict['milestones'])
            self.assertNotIn(excluded, verdict['milestones'])
            self.assertFalse(verdict['milestones'][measured]['passed'])
            if disk == 'root':
                self.assertNotIn('ramdisk-root', verdict['milestones'])

    def test_sol_console_boot_is_not_short_for_stages_only_the_keyboard_produces(self):
        import metal_score
        root = TOOLS.parents[1]
        directory = m.STATE / 'boot'
        for name in ('docs/metal/boot-path.json',
                     'tools/breenix-runs/Sources/BreenixRuns/Resources/boot-stages-x86_64.json'):
            dest = directory / 'definition' / name
            dest.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(root / name, dest)
        (directory / 'events.jsonl').write_text('')
        m.write_json(directory / 'run.json', {'mode': 'console', 'input': 'sol'})
        lines = ['[usb] boot keyboard attached: port=2', '[usb] boot keyboard detached: port=2',
                 '[usb] boot keyboard attached: port=2', '[init] getty on the console', 'Breenix login: root',
                 'Login incorrect', 'metal-login-ok 42', 'fbcon check: shadow and screen identical, 9 glyph cells',
                 'fbcon row: metal-login-ok 42', '[getty] console acquired']
        for missing, short in ((None, False), ('fbcon row: metal-login-ok 42', True)):
            (directory / 'serial.txt').write_text('\n'.join(line for line in lines if line != missing))
            verdict = metal_score.score(directory)
            console = {name: verdict['milestones'][name] for name in ('usb-keyboard', 'console-login')}
            self.assertEqual({name: result['inputUnmeasured'] for name, result in console.items()},
                             {'usb-keyboard': 2, 'console-login': 1})
            self.assertFalse(any(result['passed'] for result in console.values()))
            self.assertEqual(metal_score.falls_short({'captureComplete': True, 'milestones': console}), short)


class ModeApplicabilityTest(StateTest):
    def setUp(self):
        super().setUp()
        root = TOOLS.parents[1]
        self.definition = json.loads((root / 'docs/metal/boot-path.json').read_text())
        self.fbcon = next(item for item in self.definition['milestones'] if item['id'] == 'fb-console')

    def test_definition_and_driver_measure_fbcon_only_when_console_keeps_screen(self):
        import metal_score
        fb_markers = [(stage['marker'], 'contains') for stage in self.fbcon['stages']['x86_64']]
        self.assertEqual(len(fb_markers), 4)
        for shape in ('ramdisk', 'install', 'disk-root', 'persistent'):
            for mode in self.definition['modes']:
                with self.subTest(shape=shape, mode=mode):
                    applies = mode in ('default', 'shell', 'console', 'persistent')
                    self.assertEqual(metal_score.measures(self.fbcon, shape, mode), applies)
                    groups = boot.measured_stages({'milestones': [self.fbcon], 'gateArch': 'x86_64'},
                                                  mode, {}, shape)
                    self.assertEqual(groups, [fb_markers] if applies else [])

    def test_final_marker_skips_mode_excluded_kernel_milestone(self):
        definition = {'gateArch': 'x86_64', 'milestones': [
            {'kernel': True, 'bootShapes': ['ramdisk'],
             'stages': {'x86_64': [{'marker': 'kernel ready'}]}}, self.fbcon]}
        self.assertEqual(boot.final_marker(definition, 'probe', {}), ('kernel ready', 'contains'))
        self.assertEqual(boot.final_marker(definition, 'shell', {}),
                         ('[fbcon] screen read back identical: ', 'contains'))

    def test_scorer_omits_inapplicable_milestone_and_keeps_four_stage_requirement(self):
        import metal_score
        directory = m.STATE / 'boot'
        definition = directory / 'definition'
        path = definition / 'docs/metal/boot-path.json'
        path.parent.mkdir(parents=True)
        m.write_json(path, {'gateArch': 'x86_64', 'milestones': [self.fbcon]})
        catalog = definition / 'tools/breenix-runs/Sources/BreenixRuns/Resources/boot-stages-x86_64.json'
        catalog.parent.mkdir(parents=True)
        m.write_json(catalog, {'stages': []})
        (directory / 'events.jsonl').write_text('')
        stages = self.fbcon['stages']['x86_64']
        for disk in (None, 'install', 'root', 'persistent'):
            for mode in self.definition['modes']:
                for count in (2, 4):
                    with self.subTest(disk=disk, mode=mode, count=count):
                        m.write_json(directory / 'run.json', {'disk': disk, 'mode': mode})
                        (directory / 'serial.txt').write_text('\n'.join(stage['marker'] for stage in stages[:count]))
                        verdict = metal_score.score(directory)
                        applies = mode in ('default', 'shell', 'console', 'persistent')
                        self.assertEqual(verdict['milestones'], {'fb-console': {
                            'reached': count, 'total': 4, 'passed': count == 4}} if applies else {})
                        self.assertEqual(metal_score.falls_short(verdict), applies and count < 4)


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

    def test_shell_sends_challenge_once_at_partial_prompt_and_requires_whole_answer(self):
        d = self.driver
        d.args.mode = 'shell'
        d.final_marker = ('metal-console-ok 42', 'line')
        d.on_line('metal-console-ok 42')  # unsolicited text is not an answer
        raw = d.dir / 'serial.raw'
        d.on_line('[init] shell ready')
        raw.write_bytes(b'bsh /> ')
        with patch.object(boot.subprocess, 'run') as send:
            d.pump_serial()
            self.assertTrue(d.shell_sent)
            self.assertEqual(send.call_count, 2)
            self.assertIn('print("metal-console-ok", 6*7)', send.call_args_list[0].args[0])
            d.pump_serial()
            self.assertEqual(send.call_count, 2)
        d.on_line('print("metal-console-ok", 6*7)')
        d.on_line('bsh /> metal-console-ok 42')
        self.finish(lambda: d.on_line('metal-console-ok 42'), m.EXIT_DONE)

    def test_install_records_a_verified_write_only_when_the_kernel_reports_the_staged_bytes(self):
        d = self.driver
        d.args.mode, d.args.disk, d.args.commit = 'probe', 'install', 'c' * 40
        d.delivery['root'] = {'diskWrite': {'installedBy': d.id, 'lba': 0, 'bytes': 1024, 'sha256AfterRecord': 'a' * 64}}
        self.finish(lambda: d.on_line('[storage] read back identical: 1024 bytes, sha256 after the record sector ' + 'b' * 64 + ' (3 ms)', observed_at=1), m.EXIT_SHORT)
        self.assertIsNone(m.read_json(m.STATE / 'disk.json'))
        d.event.assert_called_with('disk-write-verified', observed_at=1, match=False, bytes=1024)
        d.seen.discard('disk-write')
        d.on_disk_line('[storage] read back identical: 1024 bytes, sha256 after the record sector ' + 'a' * 64 + ' (3 ms)', 2)
        installed = m.read_json(m.STATE / 'disk.json')
        self.assertEqual((installed['installedBy'], installed['commit'], installed['verifiedAt']), (d.id, 'c' * 40, 2))

    def test_disk_root_boot_checks_the_record_against_the_last_install(self):
        d = self.driver
        d.args.mode, d.args.disk = 'probe', 'root'
        d.delivery['diskRoot'] = {'installedBy': '20261017T000013Z-metal', 'bytes': 1024, 'sha256AfterRecord': 'a' * 64}
        d.on_disk_line('[storage] root record: installed by boot 20261017T000013Z-metal, 1024 bytes, sha256 ' + 'a' * 64, 3)
        d.event.assert_called_with('disk-root-record', observed_at=3, matchesLastInstall=True,
                                   installedBy='20261017T000013Z-metal')

    def test_truncated_storage_prefix_cannot_complete_a_boot(self):
        d = self.driver
        d.args.disk = 'install'
        d.final_marker = ('[storage] read back identical: ', 'contains')
        d.on_line('[storage] read back identical: truncated')
        self.assertFalse(d.completion_reported)

    def test_disk_root_stages_the_requested_build_before_assembling_the_payload(self):
        d = self.driver
        d.args.disk, d.args.commit = 'root', 'b' * 40
        inputs = d.stage / 'inputs'
        inputs.mkdir(parents=True)
        (inputs / 'ready').touch()
        m.write_json(inputs / 'build.json', {'commit': d.args.commit, 'profile': 'production', 'features': []})
        with patch.object(boot.metal_image, 'extract_esp', return_value={}), \
             patch.object(boot.metal_disk, 'stage', side_effect=ValueError('changed build')) as stage:
            with self.assertRaisesRegex(ValueError, 'changed build'):
                d.prepare_payload()
        self.assertEqual(stage.call_args.args[3], d.args.commit)
        self.assertNotIn('diskRoot', d.delivery)

    def test_disk_mismatch_rejects_the_boot_before_prefix_completion(self):
        d = self.driver
        for action, line in [
            ('install', '[storage] read back identical: 2048 bytes, sha256 after the record sector ' + 'a' * 64),
            ('root', '[storage] root record: installed by boot wrong-install, 1024 bytes, sha256 ' + 'a' * 64),
        ]:
            d.args.disk = action
            d.delivery = {'root': {'diskWrite': {'bytes': 1024, 'sha256AfterRecord': 'a' * 64}},
                          'diskRoot': {'installedBy': 'correct-install', 'bytes': 1024, 'sha256AfterRecord': 'a' * 64}}
            d.final_marker = (line.split(':')[0], 'contains')
            self.finish(lambda: d.on_line(line), m.EXIT_SHORT)
            self.assertFalse(d.completion_reported)

    def test_storage_boots_wait_only_for_what_their_shape_can_produce(self):
        root = TOOLS.parents[1]
        metal = json.loads((root / 'docs/metal/boot-path.json').read_text())
        catalog = json.loads((root / 'tools/breenix-runs/Sources/BreenixRuns/Resources/boot-stages-x86_64.json').read_text())
        markers = lambda groups: {marker for group in groups for marker, _ in group}
        install = boot.measured_stages(metal, 'probe', catalog, 'install')
        disk_root = boot.measured_stages(metal, 'probe', catalog, 'disk-root')
        self.assertIn('[storage] read back identical: ', markers(install))
        self.assertIn('[block] ramdisk ', markers(install))
        self.assertNotIn('[fs] root mounted from the installed disk', markers(install))
        self.assertIn('[fs] root mounted from the installed disk', markers(disk_root))
        self.assertNotIn('[block] ramdisk ', markers(disk_root))
        self.assertNotIn('[storage] wrote ', markers(disk_root))
        self.assertIn('[ahci] first sectors read by MSI: ', markers(install) & markers(disk_root))
        self.assertEqual(boot.final_marker(metal, 'probe', catalog, 'install'),
                         ('[storage] read back identical: ', 'contains'))
        self.assertEqual(boot.final_marker(metal, 'probe', catalog, 'disk-root'),
                         ('PROBE dir OK', 'contains'))
        self.assertEqual(boot.final_marker(metal, 'default', catalog, 'ramdisk'),
                         ('Breenix login:', 'contains'))

    def test_mode_completion_waits_for_delayed_kernel_producers(self):
        d = self.driver
        d.final_marker = ('mode done', 'line')
        d.required_stages = [[('cpu started', 'line'), ('timer measured', 'line')]]
        d.on_line('mode done')
        d.on_line('timer measured')  # out of order cannot count
        d.on_line('cpu started')
        self.assertEqual(d.required_stages, [[('timer measured', 'line')]])
        self.finish(lambda: d.on_line('timer measured'), m.EXIT_DONE)

    def test_probe_waits_for_done_and_kernel_stages_and_refuses_failed_report(self):
        d = self.driver
        d.args.mode = 'probe'
        definition = d.dir / 'definition/docs/metal'
        definition.mkdir(parents=True)
        m.write_json(definition / 'boot-path.json', {
            'gateArch': 'x86_64', 'fatal': [], 'milestones': [
                {'id': 'network', 'kernel': True, 'stages': {'x86_64': [{'marker': 'DNS answer'}]}},
                {'id': 'fetch', 'stages': {'probe': [{'marker': 'PROBE metal-fetch OK'}]}}
            ]})
        d.load_definition()
        self.assertEqual(d.final_marker, ('PROBE DONE ', 'contains'))
        d.on_line('PROBE metal-fetch OK')
        d.on_line('PROBE DONE passed=17 failed=0')
        self.assertFalse(d.completion_reported)
        self.finish(lambda: d.on_line('DNS answer'), m.EXIT_DONE)
        d.completion_reported = False
        self.finish(lambda: d.on_line('PROBE DONE passed=16 failed=1'), m.EXIT_SHORT)

    def test_one_suite_ends_at_its_ready_after_the_memory_record(self):
        d = self.driver
        d.args.mode, d.args.suite = 'suite', 'processes'
        definition = d.dir / 'definition/docs/metal'
        definition.mkdir(parents=True)
        m.write_json(definition / 'boot-path.json', {'gateArch': 'x86_64', 'fatal': [], 'milestones': []})
        d.load_definition()
        d.on_line('SUITE processes DONE passed=154 failed=0 skipped=0 total=154')
        d.on_line('SUITE processes MEMORY reclaimed=PASS free_before=9 free_after=9 allowance=64')
        self.assertFalse(d.completion_reported)
        self.finish(lambda: d.on_line('SUITE_SEQUENCE READY processes'), m.EXIT_DONE)

    def test_suite_answers_each_host_key_record_once_with_boot_reports_on_the_serial_console(self):
        d = self.driver
        d.args.mode, d.args.suite = 'suite', 'console'
        d.console_send = Mock()
        record = 'SUITE console HOSTKEY keys/alt-fn seq=4 keys=alt-f2,a'
        with patch.object(boot.hostkeys.time, 'sleep'):
            d.on_line(record)  # before the kernel starts: not a case's record
            self.assertFalse(d.console_send.called)
            d.kernel_started = 120
            d.on_line(record)
            d.on_line(record)  # a serial replay of the same record is answered once
            d.on_line('SUITE console HOSTKEY keys/alt-fn seq=5 keys=alt-f2')
        self.assertEqual([c.args[0] for c in d.console_send.call_args_list], [
            'BXKEY 0400000000000000,04003b0000000000,0400000000000000,0000000000000000',
            'BXKEY 0000040000000000,0000000000000000',
            'BXKEY 0400000000000000,04003b0000000000,0400000000000000,0000000000000000'])
        logged = [c.kwargs['message'] for c in d.event.call_args_list if c.args[0] == 'hostkeys']
        self.assertEqual(len(logged), 2)
        self.assertIn('seq=4 keys=alt-f2,a sent via xhci-replay', logged[0])
        # A console boot types no host keys.
        d.args.mode, d.hostkey_watcher = 'console', None
        d.console_send.reset_mock()
        d.on_line('SUITE console HOSTKEY keys/alt-fn seq=6 keys=alt-f2')
        self.assertFalse(d.console_send.called)

    def test_failed_probe_keeps_failure_and_waits_for_kernel_producers(self):
        d = self.driver
        d.args.mode = 'probe'
        d.final_marker = ('PROBE DONE ', 'contains')
        d.required_stages = [[('DNS answer', 'line')]]
        d.on_line('PROBE DONE passed=16 failed=1')
        self.assertFalse(d.completion_reported)
        self.finish(lambda: d.on_line('DNS answer'), m.EXIT_SHORT)

    def test_final_marker_cannot_override_a_fatal(self):
        d = self.driver
        d.final_marker = ('done', 'line')
        d.fatal_patterns = [re.compile('fatal')]
        d.kernel_started = 120
        d.on_line('fatal')
        d.on_line('done')
        with patch.object(m, 'now', return_value=d.fatal_at + boot.FATAL_GRACE_SECONDS):
            self.finish(d.check_deadlines, m.EXIT_SHORT)

    def test_suite_scoring_is_bounded_and_has_no_management_environment(self):
        d = self.driver
        d.args.mode, d.args.suite = 'suite', 'smoke'
        definition = d.dir / 'definition'
        for name in ('scripts/suite-verdict.py', 'docs/boot-path.json', 'docs/suites/smoke.json'):
            path = definition / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text('fixture')
        (d.dir / 'serial.txt').write_text('fixture')
        def scorer(command, **options):
            self.assertEqual(options['timeout'], boot.SUITE_VERDICT_SECONDS + 5)
            self.assertEqual(options['env'], {'PATH': os.defpath, 'LANG': 'C.UTF-8'})
            self.assertIn('--property=DynamicUser=yes', command)
            self.assertIn('--property=PrivateNetwork=yes', command)
            self.assertIn('--property=RestrictAddressFamilies=AF_UNIX', command)
            self.assertTrue(any(arg.startswith('--property=BindReadOnlyPaths=') for arg in command))
            self.assertIn('--property=RuntimeMaxSec=' + str(boot.SUITE_VERDICT_SECONDS), command)
            self.assertIn('--property=InaccessiblePaths=/etc/breenix-metal /var/lib/breenix-metal /run/breenix-metal', command)
            raise subprocess.TimeoutExpired(command, options['timeout'])
        with patch.object(boot.subprocess, 'run', side_effect=scorer):
            self.assertFalse(d.score_suite())
        self.assertIn('timed out', (d.dir / 'suite-verdict.txt').read_text())

    def test_milestone_scoring_receives_the_boot_commits_path(self):
        d = self.driver
        d.args.mode, d.args.suite, d.args.milestone = 'suite', 'fixture', 'fixture/first'
        repo = TOOLS.parents[1]
        definition = d.dir / 'definition'
        for source, target in ((repo / 'scripts/suite-verdict.py', 'scripts/suite-verdict.py'),
                               (repo / 'docs/boot-path.json', 'docs/boot-path.json'),
                               (repo / 'tests/fixtures/suite-verdict/manifest.json', 'docs/suites/fixture.json')):
            path = definition / target
            path.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(source, path)
        path = definition / 'docs/efforts/path.json'
        path.parent.mkdir(parents=True)
        path.write_text(json.dumps(dict(efforts=[dict(id='fixture', milestones=[
            dict(id='first', measure=dict(suite='fixture', categories=['first']))])])))
        shutil.copyfile(repo / 'tests/fixtures/suite-verdict/all-pass.txt', d.dir / 'serial.txt')
        real_run = subprocess.run
        def scorer(command, **options):
            binding = next(arg.split('=', 2)[2] for arg in command if arg.startswith('--property=BindReadOnlyPaths='))
            root, sandbox = binding.split(':')
            start = command.index(sys.executable)
            args = [arg.replace(sandbox, root) for arg in command[start:]]
            self.assertEqual(args[-2:], ['--milestone', 'fixture/first'])
            return real_run(args, **options)
        with patch.object(boot.subprocess, 'run', side_effect=scorer):
            self.assertTrue(d.score_suite())
        self.assertIn('milestone categories=first passed=2 total=2', (d.dir / 'suite-verdict.txt').read_text())

    def test_suite_scoring_only_runs_after_confirmed_power_off(self):
        d = self.driver
        d.args.mode, d.args.suite = 'suite', 'smoke'
        for confirmed, expected in [(True, m.EXIT_DONE), (False, m.EXIT_INCOMPLETE)]:
            order = []
            def off(hung):
                order.append('off')
                return confirmed
            def score():
                self.assertEqual(order, ['off'])
                order.append('score')
                return True
            with patch.object(d, 'create_record'), patch.object(d, 'claim'), \
                 patch.object(d, 'prepare_payload'), patch.object(d, 'preflight'), \
                 patch.object(d, 'do_stage'), patch.object(d, 'start_console'), \
                 patch.object(d, 'power_on'), patch.object(d, 'pump_serial'), \
                 patch.object(d, 'watch', side_effect=boot.Finish(m.EXIT_DONE, 'done')), \
                 patch.object(d, 'power_off', side_effect=off), \
                 patch.object(d, 'score_suite', side_effect=score), patch.object(d, 'cleanup'):
                self.assertEqual(d.run(), expected)
            self.assertEqual(order, ['off', 'score'] if confirmed else ['off'])

    def test_scorer_launch_failure_still_releases_the_boot(self):
        d = self.driver
        d.args.mode, d.args.suite = 'suite', 'smoke'
        with patch.object(d, 'create_record'), patch.object(d, 'claim'), \
             patch.object(d, 'prepare_payload'), patch.object(d, 'preflight'), \
             patch.object(d, 'do_stage'), patch.object(d, 'start_console'), \
             patch.object(d, 'power_on'), patch.object(d, 'pump_serial'), \
             patch.object(d, 'watch', side_effect=boot.Finish(m.EXIT_DONE, 'done')), \
             patch.object(d, 'power_off', return_value=True), \
             patch.object(d, 'score_suite', side_effect=OSError('fixture')), \
             patch.object(d, 'cleanup') as cleanup:
            self.assertEqual(d.run(), m.EXIT_SHORT)
        cleanup.assert_called_once_with(m.EXIT_SHORT)

    def test_cancelled_sequence_keeps_cancellation_without_a_readback_boot(self):
        d = self.driver
        d.args.mode, d.args.suite = 'suite', 'smoke,files-io'
        d.kernel_started = 110
        with patch.object(d, 'create_record'), patch.object(d, 'claim'), \
             patch.object(d, 'prepare_payload'), patch.object(d, 'preflight'), \
             patch.object(d, 'do_stage'), patch.object(d, 'start_console'), \
             patch.object(d, 'power_on'), patch.object(d, 'pump_serial'), \
             patch.object(d, 'watch', side_effect=boot.Finish(m.EXIT_STOPPED, 'operator stop')), \
             patch.object(d, 'power_off', return_value=True), \
             patch.object(d, 'score_suite') as score, \
             patch.object(d, 'continue_disk_readback') as readback, patch.object(d, 'cleanup'):
            self.assertEqual(d.run(), m.EXIT_STOPPED)
        score.assert_not_called()
        readback.assert_not_called()

    def test_linux_completion_requires_checked_census(self):
        import metal_census
        from metal_census_test import capture
        d = self.driver
        d.census = metal_census.Census()
        lines, _ = capture()
        for line in lines[:-1]:
            d.on_line(line)
        self.assertIsNotNone(d.kernel_started)
        self.finish(lambda: d.on_line(lines[-1]), m.EXIT_DONE)
        d.census = metal_census.Census()
        d.on_line('metal-linux-start')
        self.finish(lambda: d.on_line('metal-linux-ok'), m.EXIT_SHORT)

    def test_linux_evidence_write_failure_still_closes_record(self):
        d = self.driver
        d.census = Mock()
        d.census.write.side_effect = OSError('fixture storage failure')
        with patch.object(d, 'create_record'), patch.object(d, 'claim'), \
             patch.object(d, 'prepare_payload'), patch.object(d, 'preflight'), \
             patch.object(d, 'do_stage'), patch.object(d, 'start_console'), \
             patch.object(d, 'power_on'), patch.object(d, 'watch', side_effect=boot.Finish(m.EXIT_DONE, 'done')), \
             patch.object(d, 'power_off', return_value=True) as off, patch.object(d, 'cleanup') as cleanup:
            self.assertEqual(d.run(), m.EXIT_INCOMPLETE)
        off.assert_called_once_with(False)
        cleanup.assert_called_once_with(m.EXIT_INCOMPLETE)

    def test_linux_payload_rejects_tampered_efi_before_staging(self):
        d = self.driver
        d.args.payload = 'linux'
        inputs = d.stage / 'inputs'
        inputs.mkdir(parents=True)
        (inputs / 'ready').touch()
        (inputs / 'metal-linux.efi').write_bytes(b'changed')
        m.write_json(inputs / 'build.json', {'files': {'efi': {'size': 7, 'sha256': 'bad'}}})
        self.finish(d.prepare_payload, m.EXIT_CANCELLED)
        self.assertIsNone(d.payload_facts)

    def test_linux_payload_checks_config_and_retains_manifest(self):
        d = self.driver
        d.args.payload = 'linux'
        inputs = d.stage / 'inputs'
        inputs.mkdir(parents=True)
        (inputs / 'ready').touch()
        import metal_census
        image = b'MZfixture'
        config = ('\n'.join('CONFIG_' + name + '=y' for name in metal_census.REQUIRED_CONFIG) +
                  '\nCONFIG_INITRAMFS_SOURCE="fixture"\n').encode()
        (inputs / 'metal-linux.efi').write_bytes(image)
        (inputs / 'config').write_bytes(config)
        manifest = {'files': {'efi': {'size': len(image), 'sha256': hashlib.sha256(image).hexdigest()},
                              'config': {'size': len(config), 'sha256': 'bad'}}}
        m.write_json(inputs / 'build.json', manifest)
        self.finish(d.prepare_payload, m.EXIT_CANCELLED)
        manifest['files']['config']['sha256'] = hashlib.sha256(config).hexdigest()
        m.write_json(inputs / 'build.json', manifest)
        d.prepare_payload()
        self.assertEqual(d.payload_facts['contentType'], 'application/efi')
        self.assertEqual((d.dir / 'linux-config').read_bytes(), config)
        self.assertEqual(m.read_json(d.dir / 'linux-build.json'), manifest)

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

    def test_suite_deadline_and_attempt_ceiling_stop_unfinished_boots(self):
        d = self.driver
        d.args.mode = 'suite'
        d.request_at = d.complete_at = d.kernel_started = 110
        d.last_serial = 530
        with patch.object(m, 'now', return_value=530):
            d.check_deadlines()
        with patch.object(m, 'now', return_value=531):
            self.finish(d.check_deadlines, m.EXIT_SHORT)
        d.lease_ceiling = d.t0 + boot.ceiling_seconds(d.args)
        d.kernel_started = d.last_serial = 1419
        with patch.object(m, 'now', return_value=1419):
            d.check_deadlines()
        with patch.object(m, 'now', return_value=1420):
            self.assertTrue(self.finish(d.check_deadlines, m.EXIT_HUNG).hung)

    def test_each_suite_of_a_multi_suite_boot_gets_its_own_deadline(self):
        d = self.driver
        d.args.mode, d.args.suite = 'suite', 'directories,processes,time'
        self.assertEqual(boot.ceiling_seconds(d.args), boot.CEILING_SECONDS['suite'] + 2 * boot.MODE_DEADLINE_SECONDS['suite'])
        d.t0 = d.request_at = d.complete_at = d.kernel_started = 110
        d.last_serial = 900
        d.lease_ceiling = 2000
        d.on_line('SUITE processes START cases=154', observed_at=500)
        d.on_line('SUITE processes CASE fork PASS ms=1', observed_at=910)
        self.assertEqual(d.lease_ceiling, 2000)
        with patch.object(m, 'now', return_value=920):
            d.check_deadlines()
        with patch.object(m, 'now', return_value=921):
            self.finish(d.check_deadlines, m.EXIT_SHORT)

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

    def redactor_for_events(self):
        return m.Redactor(m.load_site(FIXTURES / 'redact-site.env'))

    def test_host_events_keep_source_times_after_delayed_polling(self):
        d = self.driver
        d.event = boot.Driver.event.__get__(d, boot.Driver)
        d.redact = self.redactor_for_events()
        with patch.object(m, 'now', return_value=130), patch.object(m, 'record_post_time'):
            d.on_http({'event': 'start', 't': 110, 'id': d.id})
            d.on_http({'event': 'complete', 't': 111, 'id': d.id, 'matches': True, 'sha256': '3a' * 32})
            d.on_line(boot.HANDOFF_LINE, observed_at=112)
        events = [json.loads(line) for line in (d.dir / 'events.jsonl').read_text().splitlines()]
        stamps = {event['event']: event['t'] for event in events}
        self.assertEqual(stamps['image-request'], 110)
        self.assertEqual(stamps['image-complete'], 111)
        self.assertEqual(stamps['handoff'], 112)
        self.assertEqual(d.kernel_started, 112)
        complete = next(event for event in events if event['event'] == 'image-complete')
        self.assertEqual(complete['seconds'], 11)
        self.assertEqual(complete['sha256'], '3a' * 32)

    def test_power_on_uses_queue_dispatch_time(self):
        d = self.driver
        d.event = boot.Driver.event.__get__(d, boot.Driver)
        self.ready()
        m.claim_lease({'id': d.id, 'pid': os.getpid()}, wait=False)
        with patch.object(m, 'rf', return_value={'startedAt': 105, 'completedAt': 106}), \
             patch.object(m, 'rf_get', return_value={'Boot': {'BootSourceOverrideEnabled': 'Once',
                                                           'BootSourceOverrideTarget': 'UefiHttp'}}), \
             patch.object(m, 'now', return_value=108):
            d.power_on()
        self.assertEqual(d.t0, 105)
        self.assertEqual(m.read_lease()['powerOnAt'], 105)
        events = [json.loads(line) for line in (d.dir / 'events.jsonl').read_text().splitlines()]
        self.assertEqual(next(event['t'] for event in events if event['event'] == 'power-on'), 105)

    def test_reset_detection_uses_serial_and_boot_progress(self):
        d = self.driver
        d.on_line(boot.BOOTLOADER_LINE)
        d.on_line(boot.HANDOFF_LINE)
        self.assertIsNotNone(d.kernel_started)
        self.finish(lambda: d.on_line(boot.BOOTLOADER_LINE), m.EXIT_SHORT)
        for state in boot.RUNNING_STATES:
            d.bmc_events = [{'t': 120, 'progress': {'LastState': state}}]
            d.pump_bmc()
        d.bmc_events = [{'t': 120, 'progress': {'LastState': 'MemoryInit'}}]
        self.finish(d.pump_bmc, m.EXIT_SHORT)

    def test_default_waits_for_the_unterminated_getty_prompt(self):
        d = self.driver
        d.final_marker = ("Breenix login:", "contains")
        d.required_stages = [[("[init] Boot script completed", "contains"), d.final_marker]]
        raw = d.dir / "serial.raw"
        raw.write_bytes(b"[init] Boot script completed\nBreenix log")
        d.pump_serial()
        self.assertFalse(d.final_seen)
        with raw.open("ab") as stream:
            stream.write(b"in: ")
        self.finish(d.pump_serial, m.EXIT_DONE)
        self.assertFalse(any(d.required_stages))
        self.assertTrue((d.dir / "serial.txt").read_text().endswith("Breenix login: "))

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
        definition = {'milestones': [{'bootShapes': ['ramdisk'], 'stages': {'default': [{'marker': 'DONE|OK', 'match': 'line'}]}}]}
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

    def test_nmi_option_sends_the_nmi_before_powering_off_a_finished_kernel(self):
        d = self.driver
        d.kernel_started = 105
        sent = []
        with patch.object(m, 'rf', side_effect=lambda method, path, body, **kw: sent.append(body['ResetType'])), \
             patch.object(m, 'force_off_and_wait', return_value=True), patch.object(boot, 'NMI_CAPTURE_SECONDS', 0):
            self.assertTrue(d.power_off(False))
            self.assertEqual(sent, [], 'a finished boot gets no NMI by default')
            d.args.nmi = True
            self.assertTrue(d.power_off(False))
        self.assertEqual(sent, ['Nmi'])

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

    def test_preflight_rejects_usb_and_video_drift(self):
        site = m.load_site(FIXTURES / 'redact-site.env')
        site.update(INTERNAL_USB='Off', USB_MANAGED_PORT='Off')
        expected = m.expected_bios(site)
        self.assertEqual({key: expected[key] for key in
                          ('UsbPorts', 'GenericUsbBoot', 'EmbVideo', 'InternalUsb', 'UsbManagedPort')},
                         {'UsbPorts': 'AllOn', 'GenericUsbBoot': 'Disabled', 'EmbVideo': 'Enabled',
                          'InternalUsb': 'Off', 'UsbManagedPort': 'Off'})
        def get(path):
            if path == m.SYSTEM + '/Bios':
                return {'Attributes': dict(expected, **m.KEPT_BIOS,
                                           UsbPorts='AllOff', GenericUsbBoot='Enabled', EmbVideo='Disabled')}
            if path == m.MANAGER_ATTRIBUTES:
                return {'Attributes': m.EXPECTED_MANAGER}
            if path == m.LC_ATTRIBUTES:
                return {'Attributes': m.EXPECTED_LC}
            return {'Boot': {'BootSourceOverrideEnabled': 'Disabled'}}
        with patch.object(m, 'rf_get', side_effect=get):
            rows, _ = m.machine_checks(site)
        self.assertEqual([name for name, _, _ in m.failed_checks(rows)],
                         ['bios.UsbPorts', 'bios.GenericUsbBoot', 'bios.EmbVideo'])

    def test_sata_off_policy_is_checked_explicitly(self):
        site = m.load_site(FIXTURES / 'redact-site.env')
        site['SATA_MODE'] = 'Off'
        self.assertEqual(m.expected_bios(site)['EmbSata'], 'Off')

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


class SetupOriginalsTest(StateTest):
    def test_original_receipt_retains_pending_values_including_no_ops(self):
        setup = load_script('metal-setup')
        current = {'UsbPorts': 'AllOff', 'GenericUsbBoot': 'Disabled', 'EmbVideo': 'Enabled'}
        pending = {'UsbPorts': 'BackOnly', 'EmbVideo': 'Enabled'}
        path = setup.save_originals('fixture-setup', current, pending,
                                    {'IPMILan.1.Enable': 'Enabled', 'Users.1.Password': 'not-copied'},
                                    m.EXPECTED_LC, {'UsbPorts': 'AllOn'})
        saved = m.read_json(path)
        self.assertEqual(saved['bios'], current)
        self.assertEqual(saved['pending'], pending)
        self.assertEqual(saved['requested'], {'UsbPorts': 'AllOn'})
        self.assertEqual(saved['manager'], {'IPMILan.1.Enable': 'Enabled'})
        self.assertEqual(saved['lc'], m.EXPECTED_LC)
        self.assertEqual(path.stat().st_mode & 0o777, 0o600)

    def test_apply_preserves_unapplied_bios_changes(self):
        setup = load_script('metal-setup')
        site = m.load_site(FIXTURES / 'redact-site.env')
        self.ready()
        current = dict(m.expected_bios(site), UsbPorts='AllOff')
        pending = {'EmbVideo': 'Disabled'}
        def get(path):
            if path == m.SYSTEM + '/Bios':
                return {'Attributes': current}
            if path == m.SYSTEM + '/Bios/Settings':
                return {'Attributes': pending}
            return {'Attributes': {}}
        with patch.object(m, 'load_site', return_value=site), patch.object(m, 'rf_get', side_effect=get), \
             patch.object(m, 'rf') as write, patch.object(m, 'force_off_and_wait', return_value=True), \
             patch.object(m, 'clear_override'):
            with self.assertRaisesRegex(SystemExit, 'unapplied BIOS settings'):
                setup.apply(SimpleNamespace(refresh=False))
        self.assertFalse(any(call.args[1] == m.SYSTEM + '/Bios/Settings' for call in write.call_args_list))
        originals = list((m.STATE / 'setup').glob('*-original.json'))
        self.assertEqual(len(originals), 1)
        self.assertEqual(m.read_json(originals[0])['pending'], pending)
        self.assertIsNone(m.read_lease())


class StagingTest(unittest.TestCase):
    def test_build_pushes_only_stub_sources_and_replaces_stage(self):
        run = load_script('metal-run')
        env = dict(BUILD_ROOT='/fixture', BUILD_INSTANCE='builder', METAL_INSTANCE='service',
                   BUILD_REPO='/repo', BUILD_RUST_FORK='/rust', BUILD_CACHE='/cache')
        import io
        sender = Mock(stdout=io.BytesIO(), wait=Mock(return_value=0))
        with patch.object(run, 'host_env', return_value=env), \
             patch.object(run, 'in_build', return_value=SimpleNamespace(returncode=0)) as builder, \
             patch.object(run, 'in_metal', return_value=SimpleNamespace(returncode=0)) as service, \
             patch.object(run, 'incus') as push, \
             patch.object(run.subprocess, 'Popen', return_value=sender), \
             patch.object(run.subprocess, 'run', return_value=SimpleNamespace(returncode=0)):
            self.assertEqual(run.build('20261017T000009Z-metal', 'a' * 40), 0)
        pushed = [call.args[2:4] for call in push.call_args_list if call.args[:2] == ('file', 'push')]
        self.assertEqual([target.split('-tools/', 1)[1] for _, target in pushed],
                         ['metal-build', 'host-slots.py', 'gate-artifacts.py', 'gate-tree.py', 'effort-target-guard.py', 'efi-stub/Cargo.toml',
                          'efi-stub/Cargo.lock', 'efi-stub/rust-toolchain.toml', 'efi-stub/src/bootdisk.rs',
                          'efi-stub/src/com1.rs', 'efi-stub/src/files.rs', 'efi-stub/src/http.rs', 'efi-stub/src/lib.rs',
                          'efi-stub/src/main.rs', 'efi-stub/src/manager/clock.rs', 'efi-stub/src/manager/lab.rs',
                          'efi-stub/src/manager/main.rs', 'efi-stub/src/manager/options.rs',
                          'efi-stub/src/manager/screen.rs', 'efi-stub/src/manager/slots.rs',
                          'efi-stub/src/persist.rs', 'efi-stub/src/slot_transaction.rs',
                          'efi-stub/src/suite_disk.rs', 'efi-stub/src/uefi.rs'])
        self.assertTrue(all(Path(source).is_file() for source, _ in pushed))
        calls = [call.args[1:] for call in service.call_args_list]
        stage = '/var/lib/breenix-metal/stage/20261017T000009Z-metal/inputs'
        self.assertLess(calls.index(('rm', '-rf', stage)), calls.index(('mkdir', '-p', stage)))
        self.assertEqual(builder.call_args_list[0].args[1:3], ('rm', '-rf'))


class CatchupTest(StateTest):
    def test_catchup_is_recorded_and_defaults_to_false(self):
        for catchup in (False, True):
            args = SimpleNamespace(id='20261017T00000' + str(int(catchup)) + 'Z-metal',
                                   mode='default', payload='image', target='main', task='',
                                   commit='a' * 40, inject=None, suite=None, nmi=False, catchup=catchup)
            with patch.object(m, 'load_site', return_value={}):
                driver = boot.Driver(args)
            driver.create_record()
            record = m.read_json(driver.dir / 'run.json')
            self.assertIs(record['catchup'], catchup)

    def test_environment_matches_run_sh(self):
        run = load_script('metal-run')
        for value, expected in [('1', True), ('YES', True), ('true', True),
                                ('0', False), ('no', False), ('false', False), ('', False)]:
            with patch.dict(os.environ, BREENIX_CATCHUP=value):
                self.assertIs(run.catchup_from_env(), expected)
        with patch.dict(os.environ, BREENIX_CATCHUP='maybe'):
            with self.assertRaises(SystemExit):
                run.catchup_from_env()


class BuildPolicyTest(unittest.TestCase):
    def test_guard_comes_from_origin_main_instead_of_the_target_tree(self):
        for catchup in (False, True):
            with self.subTest(catchup=catchup):
                self.guard_from_main(catchup)

    def guard_from_main(self, catchup):
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
                                   rust_fork=str(root / 'rust'), cache=str(root / 'cache'), catchup=catchup)
            self.assertEqual(build.outer(args), 3)


class BuildSlotTest(unittest.TestCase):
    def build_inner(self, suite):
        """Run metal-build's inner build with every subprocess faked; returns the helper and build calls."""
        build = load_script('metal-build')
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            args = SimpleNamespace(id='20261017T000012Z-metal', root=str(root), repo=str(root / 'repo'), sha='a' * 40,
                                   rust_fork=str(root / 'rust'), cache=str(root / 'cache'), stub=str(root / 'stub'),
                                   guard_target='x86-metal', features='', suite=suite)
            clone, out, tmp = build.paths(args)
            out.mkdir(parents=True)
            image = clone / 'target/release/build/breenix-0/out/breenix-uefi.img'
            stub = tmp / 'stub-target' / build.STUB_TARGET / 'release/metal-efi-stub.efi'
            for path in (image, stub, stub.with_name('metal-boot-manager.efi'), clone / 'testdata/ext2.img'):
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(b'bytes')
            path = clone / 'docs/efforts/path.json'
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text('fixture effort path')
            order = []
            def run(command, **options):
                order.append(command[2:])
                return SimpleNamespace(returncode=0, stdout=str(root / 'cargo-home') + '\n')
            def call(command, **options):
                order.append(command)
                if command[1:2] == [str(clone / 'scripts/sdk-suite.py')]:
                    Path(command[-1]).write_bytes(b'staged')
                    work = options['env']['TMPDIR']
                    order.append(('sdk-work', work, Path(work).is_dir()))
                return 0
            with patch.dict(build.os.environ, CARGO_BUILD_JOBS='4'), \
                 patch.object(build.subprocess, 'run', side_effect=run), \
                 patch.object(build.subprocess, 'call', side_effect=call):
                self.assertEqual(build.inner(args), 0)
            built = json.loads((out / 'build.json').read_text())
            self.assertEqual((out / 'definition/docs/efforts/path.json').read_text(), 'fixture effort path')
            built['builtRoot'] = (clone / 'testdata/ext2.img').read_bytes()
            built['clone'] = str(clone)
            return build, clone, tmp, order, (out / 'ext2.img').read_bytes(), built

    def test_one_metal_build_lease_covers_a_single_userspace_build_and_the_kernel(self):
        build, clone, tmp, order, root, built = self.build_inner('')
        helper = [call for call in order if isinstance(call, list) and call[:1] not in ([sys.executable], ['cargo'])]
        calls = [call for call in order if call not in helper]
        self.assertEqual(helper, [['cargo-home', str(Path.home() / '.cargo'), str(clone / 'target')],
                                  ['acquire', 'metal-build'], ['release', 'metal-build']])
        # The deployed helpers beside metal-build, not the commit's copies.
        self.assertEqual(calls[0][1:], [str(build.ARTIFACTS), '--metal', str(clone), str(tmp / 'logs')])
        self.assertFalse(build.ARTIFACTS.is_relative_to(clone))
        self.assertEqual(calls[1][:3], ['cargo', 'build', '--release'])
        self.assertEqual(len(calls), 3)
        self.assertEqual((root, built['suite']), (b'bytes', None))

    def test_the_sdk_suite_stages_its_root_under_the_build_lease(self):
        _, clone, _, order, root, built = self.build_inner('sdk')
        staging = next(i for i, call in enumerate(order) if call[1:2] == [str(clone / 'scripts/sdk-suite.py')])
        self.assertEqual(order[staging][2:5], ['--arch', 'x86_64', '--image'])
        self.assertLess(order.index(['acquire', 'metal-build']), staging)
        self.assertLess(staging, order.index(['release', 'metal-build']))
        # The staged copy is the root delivered; the built root is left as it is.
        self.assertEqual((root, built['builtRoot'], built['suite']), (b'staged', b'bytes', 'sdk'))
        # The harness's work folder holds neither the checkout's path nor the builder's home, which it scans for.
        _, work, existed = order[staging + 1]
        self.assertTrue(existed)
        self.assertFalse(Path(work).exists())
        self.assertNotIn(built['clone'], work)
        self.assertNotIn(str(Path.home()), work)


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
    import metal_root
    try:
        metal_root.require_e2fsprogs('mke2fs', 'debugfs')
    except ValueError as error:
        raise SystemExit(str(error)) from None
    unittest.main()
