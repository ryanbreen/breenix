import argparse
import contextlib
import json
from pathlib import Path
import runpy
import subprocess
import sys
import threading
import tempfile
import unittest
from unittest.mock import Mock, patch


class PairedFlashTest(unittest.TestCase):
    def setUp(self):
        tools = Path(__file__).resolve().parents[1] / 'scripts/metal'
        sys.path.insert(0, str(tools))
        self.addCleanup(sys.path.remove, str(tools))
        self.flash = runpy.run_path(str(tools / 'metal_live_flash.py'))
        self.current = '20261127T230258Z-metal'
        self.previous = '20261127T224742Z-metal'
        self.trial = '20261127T235000Z-metal'
        self.staged = '20261127T235900Z-metal'
        self.token = '1' * 32
        self.commit = '2' * 40

    def test_reclaim_through_each_host_publication_cut(self):
        slots = {'current': {'userland': self.current},
                 'previous': {'userland': self.previous}}
        trial = dict(userland=self.trial)
        staged = dict(userland=self.staged)
        names = lambda *ids: {'gen-' + value for value in ids}
        # An acknowledged trial can be left unpromoted, for example
        # when NEXT differs from the trial. Its receipt keeps three roots.
        for attempts, booted, expected in (
            ([], self.current, names(self.current, self.previous)),
            ([staged], self.current, names(self.current, self.previous, self.staged)),
            ([trial], self.trial, names(self.current, self.previous, self.trial)),
            ([trial, staged], self.trial, names(self.current, self.previous, self.trial, self.staged)),
        ):
            with self.subTest(booted=booted, attempts=attempts):
                self.assertEqual(self.flash['retained_roots'](slots, attempts, 'gen-' + booted),
                                 expected)
        promoted = {'current': trial, 'previous': slots['current']}
        self.assertEqual(self.flash['retained_roots'](promoted, [], 'gen-' + self.trial),
                         names(self.current, self.trial))

    def test_manifest_capacity_and_capability(self):
        entry = dict(commit=self.commit, sha256='3' * 64, baseNoteSha256='4' * 64,
                     userland=self.trial)
        for size, length in [(99_999_999, 256), (536_870_912, 257)]:
            with self.subTest(size=size):
                entry['size'] = size
                legacy = self.flash['manifest'](self.token, entry, 1)
                paired = self.flash['manifest'](self.token, entry, 2)
                self.assertEqual(len(legacy.splitlines()), 6)
                self.assertLessEqual(len(legacy), 255)
                self.assertEqual(paired.splitlines()[0], b'BREENIX-FLASH 2')
                self.assertEqual(paired.splitlines()[-1], ('gen-' + self.trial).encode())
                self.assertEqual(len(paired), length)

    def test_paired_install_uses_live_build_and_rejects_network_recovery(self):
        runner = runpy.run_path(str(Path(__file__).resolve().parents[1] / 'scripts/metal/metal-run'))
        scope = runner['install_persistent'].__globals__
        live = Mock(return_value=0)
        network = Mock(side_effect=AssertionError('paired deploy used the network installer'))
        scope.update(live_flash=live, boot=network)
        parser = runner['argument_parser']()
        args = parser.parse_args(['install-persistent', '--paired', '--inject', 'paired-no-commit'])
        self.assertEqual(runner['install_persistent'](args), 0)
        live.assert_called_once_with(args)
        args.recovery = True
        with self.assertRaises(SystemExit):
            runner['install_persistent'](args)

    def test_stage_before_begin_and_verify_only_the_selected_pair(self):
        calls = []
        path = '/root/.breenix-flash/tools/' + 'a' * 64 + '/persistent-install'
        inventory = b'inventory'

        def put(name, payload, mode):
            calls.append(('put', name, payload, mode))

        def paired(*words):
            calls.append(('paired', *words))
            return {'protocol': 'BREENIX-PAIRED 1', 'have': '', 'apply': '{"carried": [], "deletedOnBox": [], "onBox": []}',
                    'verify': 'paired verify ' + self.trial + ' rows=1 sha256=' + 'b' * 64}.get(words[0], '')

        roots = ['gen-' + self.current, 'gen-' + self.previous]
        self.flash['stage_pair'](self.trial, path, b'tool', inventory, roots, put, paired,
                                 lambda have: calls.append(('upload', have)), 'b' * 64)
        self.assertEqual(calls[:5], [
            ('put', path, b'tool', '0700'),
            ('put', '/root/.breenix-flash/' + self.trial + '/inventory.tsv', inventory, '0600'),
            ('paired', 'protocol'),
            ('paired', 'reclaim', *roots),
            ('paired', 'begin', self.trial, *roots),
        ])
        self.assertEqual([call[1] for call in calls if call[0] == 'paired'],
                         ['protocol', 'reclaim', 'begin', 'have', 'apply', 'verify', 'publish'])
        for root, mode, digest, accepted in (
            ('gen-' + self.trial, 'rw', 'b' * 64, True),
            ('gen-' + self.current, 'rw', 'b' * 64, False),
            ('gen-' + self.trial, 'ro', 'b' * 64, False),
            ('gen-' + self.trial, 'rw', 'c' * 64, False),
        ):
            with self.subTest(root=root, mode=mode, digest=digest):
                cold = Mock(return_value='paired verify ' + self.trial + ' rows=1 sha256=' + digest)
                self.assertEqual(self.flash['confirmed_pair'](('7-8-9', root, mode), '7-8-9',
                    self.trial, 'b' * 64, self.trial, cold), accepted)
                if root == 'gen-' + self.trial and mode == 'rw':
                    cold.assert_called_once_with('verify', self.trial, '--booted')

    def test_reboot_checks_pending_keys_as_well_as_boot_id(self):
        guard = self.flash['paired_reboot_guard']('7-8-9')
        self.assertIn('/run/breenix/boot-id', guard)
        self.assertIn('/var/lib/accounts/pending', guard)
        self.assertTrue(guard.endswith(' && '))

    def test_prepare_seals_the_new_root_for_both_manager_versions(self):
        globals_ = self.flash['prepare'].__globals__
        import metal_image
        import metal_persist

        class Service:
            def __init__(self, root):
                self.STATE = root

            def read_json(self, path):
                return dict(profile='production', commit='2' * 40, writablePersistentRoot=True,
                            breenfsIncompat=1) if path.name == 'build.json' else {}

            def write_json(self, path, value):
                path.write_text(json.dumps(value))

            def boot_dir(self, id):
                return self.STATE / 'boots' / id

        current = dict(userland=self.current, commit='c' * 40, installedBy='0' * 32)
        with tempfile.TemporaryDirectory() as root, contextlib.ExitStack() as mocks:
            mocks.enter_context(patch.dict(globals_, {
                'site': lambda m: dict(lease=self.current, current=current),
                'prepare_inventory': Mock(),
            }))
            for module, bindings in [
                (metal_image, {'extract_esp': Mock(return_value={})}),
                (metal_persist, {'layout': Mock(return_value={}), 'slot_image': Mock(),
                                 'store_definition': Mock(),
                                 'file_facts': Mock(return_value={'size': 536870912, 'sha256': '3' * 64})})
            ]:
                for name, value in bindings.items():
                    mocks.enter_context(patch.object(module, name, value))
            build = self.flash['prepare'](Service(Path(root)), self.trial, self.token,
                                          self.commit, self.current, False, '', paired=True)
            self.assertEqual(metal_persist.slot_image.call_args.args[4], self.trial)
            self.assertTrue(metal_persist.slot_image.call_args.kwargs['writable_root'])
            self.assertEqual(build['previousRoot'], 'gen-' + self.current)
            for version, rows in [(1, 6), (2, 7)]:
                self.assertEqual(len(self.flash['manifest'](self.token, build, version).splitlines()), rows)

    def test_atomic_host_publication_and_retention_across_cuts(self):
        # A cut before the bput rename preserves the prior committed bytes.
        # Arm and acknowledgement have their own publication points.
        fields = {'size': 99, 'sha256': 'a' * 64, 'baseNoteSha256': 'b' * 64,
                  'commit': self.commit, 'userland': self.trial}
        manifest = self.flash['manifest'](self.token, fields, 2)
        for cut in range(7):
            durable = {'committed': b'old-token\n'}
            writes = 0
            def guest(script, input, timeout):
                nonlocal writes
                self.assertIn('/bin/bput /root/.breenix-flash/', script)
                path = script.split('/root/.breenix-flash/', 1)[1].split()[0]
                if cut == writes:
                    return subprocess.CompletedProcess([], 124, b'', b'cut before rename')
                writes += 1
                durable[path] = input
                if cut == writes:
                    return subprocess.CompletedProcess([], 124, b'', b'cut after rename')
                writes += 1
                return subprocess.CompletedProcess([], 0, b'published\n', b'')

            for name, payload in [('kernel.img', b'trial image'), ('manifest', manifest),
                                  ('committed', (self.token + '\n').encode())]:
                try:
                    self.flash['publish'](guest, name, payload)
                except ValueError:
                    break
            self.assertEqual('kernel.img' in durable, cut >= 1)
            self.assertEqual('manifest' in durable, cut >= 3)
            if 'manifest' in durable:
                self.assertEqual(durable['manifest'], manifest)
            self.assertEqual(durable['committed'], (self.token + '\n').encode() if cut >= 5 else b'old-token\n')

    def test_reset_request_is_bound_to_an_unacknowledged_persistent_lease(self):
        class Service:
            def __init__(self, root, lease):
                self.STATE = root
                self.lease = lease

            def locked(self, name):
                return contextlib.nullcontext()

            def read_lease(self):
                return self.lease

            def read_json(self, path):
                return json.loads(path.read_text()) if path.is_file() else None

            def write_json(self, path, value):
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text(json.dumps(value))

            def boot_dir(self, id):
                return self.STATE / 'boots' / id

            def now(self):
                return 100

        with tempfile.TemporaryDirectory() as root:
            service = Service(Path(root), dict(id=self.current, state='powered', kind='persistent'))
            record = self.flash['record_path'](service, self.token)
            entry = dict(lease=self.current, paired=True, status='trial', armedAt=10)
            service.write_json(record, entry)
            request = self.flash['request_reset']
            with self.assertRaises(ValueError):
                request(service, self.previous, self.token)
            request(service, self.current, self.token)
            self.assertEqual(service.read_json(service.boot_dir(self.current) / 'paired-reset.json')['token'],
                             self.token)
            service.write_json(record, dict(entry, status='acknowledged'))
            with self.assertRaises(ValueError):
                request(service, self.current, self.token)

    def test_failed_trials_do_not_acknowledge_and_return_only_after_rollback(self):
        flash = self.flash['run_paired'].__globals__
        current = 'gen-' + self.current
        prior_commit = 'c' * 40
        build = dict(size=16, sha256='a' * 64, baseNoteSha256='b' * 64, commit=self.commit,
                     userland=self.trial, baseCommit=prior_commit, previousRoot=current, image='/file')
        context = dict(lease=self.current, retainedRoots=[current], manifestVersion=2)
        trial_serial = (f'breenix-manager: started: test\n'
                        f'breenix-manager: live flash: trial {self.token} {self.commit}\n'
                        '[init] log boot-id=4-5-6\n')
        rollback_serial = (f'breenix-manager: started: test\n'
                           f'breenix-manager: live flash: rollback {self.token}\n'
                           f'breenix-manager: booting current {prior_commit}\n'
                           '[init] log boot-id=7-8-9\n')
        for ssh_available in [True, False]:
            events = []
            phase = 'current'
            serial = ''
            observed_ssh = False
            def guest(script, **kwargs):
                nonlocal phase, serial
                if script.endswith('/sbin/reboot'):
                    if phase == 'current':
                        self.assertIn('/var/lib/accounts/pending', script)
                        phase = 'trial'
                        serial = trial_serial
                        events.append('initial-reboot')
                    else:
                        self.assertIn('= 4-5-6', script)
                        phase = 'rollback'
                        serial += rollback_serial
                        events.append('guarded-reboot')
                return subprocess.CompletedProcess([], 0, b'', b'')

            guest.install_uploader = lambda data: None

            def facts(_):
                nonlocal observed_ssh
                if phase == 'rollback':
                    observed_ssh = True
                    return '7-8-9', current, 'rw'
                if phase == 'trial':
                    return ('4-5-6', 'gen-wrong', 'rw') if ssh_available else None
                return '1-2-3', current, 'rw'

            def service(_, *words, **kwargs):
                nonlocal phase, serial
                value = b''
                status = 0
                if words[0] == 'python3':
                    command = words[2]
                    if command in ('site', 'prepare', 'record'):
                        value = json.dumps(context if command == 'site' else build).encode()
                    elif command == 'installer':
                        value = b'tool'
                    elif command == 'paired-reset':
                        events.append('lease-reset')
                        phase = 'rollback'
                        serial += rollback_serial
                elif words[0] == 'stat':
                    value = b'0'
                elif words[0] == 'test':
                    status = 1
                elif words[0] == 'tail':
                    value = serial
                elif words[0] == 'cat':
                    value = b'inventory'
                elif words[0] == 'tee':
                    self.assertTrue(observed_ssh)
                    self.assertEqual(json.loads(kwargs['input'])['result'], 'rolled-back')
                    events.append('receipt')
                return subprocess.CompletedProcess([], status, value, b'')

            runner = dict(host_env=lambda: {}, in_metal=service, STATE='/state', TOOLS='/tools',
                          resolve=lambda env, branch: self.commit, build=Mock(return_value=0),
                          paired_tools_commit=lambda: self.commit)
            args = argparse.Namespace(branch=None, sha=None, inject=None)
            with tempfile.TemporaryDirectory() as root, contextlib.ExitStack() as mocks:
                writer = Mock()
                mocks.enter_context(patch.dict(flash, {
                    'guest_access': lambda _: contextlib.nullcontext(guest),
                    'guest_facts': facts, 'publish': writer,
                    'stage_pair': lambda *args: dict(carried=[], deletedOnBox=[], onBox=[]),
                    'tempfile_image': lambda *args: contextlib.nullcontext(None),
                    'DEADLINE': 20,
                }))
                for key, value in [('monotonic', Mock(side_effect=range(100))),
                                   ('strftime', Mock(return_value=self.trial.removesuffix('-metal'))),
                                   ('sleep', Mock())]:
                    mocks.enter_context(patch('time.' + key, value))
                mocks.enter_context(patch('secrets.token_hex', return_value=self.token))
                mocks.enter_context(patch('pathlib.Path.home', return_value=Path(root)))
                self.assertEqual(self.flash['run_paired'](runner, args), 1)
                self.assertEqual(events, ['initial-reboot', 'guarded-reboot' if ssh_available else 'lease-reset',
                                          'receipt'])
                self.assertNotIn('committed', [call.args[1] for call in writer.call_args_list])

    def test_forced_rollback_waits_for_both_halves_of_the_old_pair(self):
        for trial_boot in (None, '4-5-6'):
            with self.subTest(trial_boot=trial_boot):
                rollback = (
                    'breenix-manager: started: test\n'
                    f'breenix-manager: live flash: rollback {self.token}\n'
                    f'breenix-manager: booting current {self.commit}\n'
                    '[init] log boot-id=7-8-9\n'
                )
                facts = Mock(side_effect=[('7-8-9', 'gen-' + self.trial, 'rw'),
                                          ('7-8-9', 'gen-' + self.current, 'rw')])
                read_serial = Mock(side_effect=['', rollback, rollback])
                reboot = Mock()
                reset = Mock()
                with patch('time.monotonic', side_effect=range(30)), patch('time.sleep'):
                    self.assertEqual(self.flash['force_rollback'](
                        self.token, '1-2-3', trial_boot, self.commit, 'gen-' + self.current,
                        read_serial, facts, reboot, reset), '7-8-9')
                self.assertEqual(facts.call_count, 2)
                if trial_boot:
                    reboot.assert_called_once_with(trial_boot)
                    reset.assert_not_called()
                else:
                    reboot.assert_not_called()
                    reset.assert_called_once_with(self.token)


class FlashPathTest(unittest.TestCase):
    def test_kernel_flash_uses_ssh_unless_explicitly_in_recovery(self):
        runner = runpy.run_path(str(Path(__file__).resolve().parents[1] / 'scripts/metal/metal-run'))
        scope = runner['install_persistent'].__globals__
        for recovery, live_result, expected in [
                (False, 0, 'live'), (False, 1, 'live'), (True, 1, 'network')]:
            with self.subTest(recovery=recovery, live_result=live_result):
                live = Mock(return_value=live_result)
                network = Mock(return_value=17)
                scope.update(live_flash=live, boot=network)
                args = argparse.Namespace(sha=None, branch=None, rollback=False, inject=None,
                                          kernel_only=True, recovery=recovery, replace_manager=False)
                result = runner['install_persistent'](args)
                self.assertEqual(result, live_result if expected == 'live' else 17)
                self.assertEqual(live.call_count, int(expected == 'live'))
                self.assertEqual(network.call_count, int(expected == 'network'))


    def test_ssh_health_uses_privileged_pid1_inspection(self):
        module = runpy.run_path(str(Path(__file__).resolve().parents[1] / 'scripts/metal/metal_live_flash.py'))
        shell = r"""
            readlink() { return 1; }
            sudo() {
                test "$*" = '-n /bin/readlink /proc/1/exe' || return 1
                printf '%s\n' "$PID1"
            }
            cat() { printf '%s\n' '12-34-56'; }
        """
        for executable in ('/sbin/init', '/bin/sh'):
            with self.subTest(executable=executable):
                result = subprocess.run(['sh', '-c', shell + module['HEALTH']],
                                        env={'PID1': executable}, capture_output=True, text=True)
                self.assertEqual(result.returncode, int(executable != '/sbin/init'))
                self.assertEqual(result.stdout, '12-34-56\n' if executable == '/sbin/init' else '')


    def test_firmware_cold_reset_keeps_the_persistent_lease_through_off(self):
        path = Path(__file__).resolve().parents[1] / 'scripts/metal/metal-boot'
        boot = runpy.run_path(str(path))
        driver = boot['Driver'].__new__(boot['Driver'])
        driver.args = argparse.Namespace(payload='persistent')
        driver.kernel_started = None
        driver.migration_phase = None
        driver.requested_reset_at = None
        driver.seen = {'live-flash-trial', 'manager-slot'}
        driver.event = Mock()
        driver.on_slot_line = Mock()
        driver.on_slot_boot = Mock()
        driver.t0 = 1
        driver.power_seen_on = True
        driver.bmc_lock = threading.Lock()
        driver.on_manager_line(
            'breenix-manager: live flash: candidate returned (0x8000000000000003); restarting',
            observed_at=100, seconds=99)
        driver.bmc_events = [{'t': 105, 'power': 'Off'}]
        driver.pump_bmc()
        self.assertEqual(driver.requested_reset_at, 100)
        self.assertNotIn('manager-slot', driver.seen)
        driver.seen.discard('live-flash-reset')
        driver.bmc_events = [{'t': 200, 'power': 'Off'}]
        with self.assertRaises(boot['Finish']):
            driver.pump_bmc()


if __name__ == '__main__':
    unittest.main()
