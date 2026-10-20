#!/usr/bin/env python3
"""Tests mode: the testing kernel's build, delivery, completion and main-path scoring; no hardware needed."""
from metal_tools_test import StateTest, boot, load_script, m
from pathlib import Path
import re
import shutil
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch
import metal_image
import metal_score


ROOT = Path(__file__).resolve().parents[1]
CONTRACTS = ('kernel/src/boot/test_list.rs', 'scripts/test-report-verdict.py', 'scripts/x86-gate-exits.py',
             'scripts/x86-gate-allowlist.txt', 'scripts/score-sigkill-teardown.py')
COUNT = len(re.findall(r'^    "([^" ]+)",', (ROOT / CONTRACTS[0]).read_text(), re.MULTILINE))
LOADER = f'[test] Loaded {COUNT}/{COUNT} test binaries (0 failed, 0 not found)'
TALLY = 'TEST_TALLY: exited=2 nonzero=0 failed=[] started=2'
REPORT = '[ INFO] kernel::syscall::handlers: USERSPACE TEST REPORT DONE'


def copy_contracts(definition):
    for name in CONTRACTS:
        target = definition / name
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(ROOT / name, target)


def run_args(**changes):
    values = dict(session=False, attended=False, wait=30, idle_minutes=30, mode='tests', input=None, payload='image',
                  disk=None, inject=None, sha=None, branch='fixture', suite=None, no_wait=False, nmi=False,
                  linux_directory=None, serve_as=None, content_type=None, ramdisk=True)
    return SimpleNamespace(**dict(values, **changes))


class TestsBuildTest(unittest.TestCase):
    def boot(self, args):
        run = load_script('metal-run')
        with tempfile.TemporaryDirectory() as home, \
             patch.object(run, 'host_env', return_value={}), \
             patch.object(run, 'resolve', return_value='a' * 40), \
             patch.object(run, 'in_metal', return_value=SimpleNamespace(returncode=0, stdout='', stderr='')) as metal, \
             patch.object(run.Path, 'home', return_value=Path(home)), \
             patch.object(run.subprocess, 'Popen') as popen:
            self.assertEqual(run.boot(args), 0)
        return metal.call_args.args, popen.call_args.args[0][2:]

    def test_milestone_selector_reaches_the_driver(self):
        run = load_script('metal-run')
        with tempfile.TemporaryDirectory() as home, \
             patch.object(run, 'host_env', return_value={}), \
             patch.object(run, 'resolve', return_value='a' * 40), \
             patch.object(run, 'resolve_suites', return_value='users'), \
             patch.object(run, 'in_metal', return_value=SimpleNamespace(returncode=0, stdout='', stderr='')) as metal, \
             patch.object(run.Path, 'home', return_value=Path(home)), \
             patch.object(run.subprocess, 'Popen'):
            self.assertEqual(run.boot(run_args(mode='default', suite='users', milestone='users/kernel-checks')), 0)
        command = metal.call_args.args
        self.assertEqual(command[command.index('--milestone') + 1], 'users/kernel-checks')
        self.assertEqual(command[command.index('--mode') + 1], 'suite')
        for suite in (None, 'all', 'users,server'):
            with self.subTest(suite=suite), self.assertRaisesRegex(SystemExit, '--milestone needs'):
                run.boot(run_args(mode='default', suite=suite, milestone='users/kernel-checks'))

    def test_a_tests_boot_requests_the_testing_kernel_on_a_disk_root(self):
        launch, build = self.boot(run_args())
        self.assertEqual(build, ['_build', build[1], 'a' * 40, '--features', 'testing'])
        self.assertEqual(launch[launch.index('--mode') + 1], 'tests')
        self.assertEqual(launch[launch.index('--disk') + 1], 'root')
        _, build = self.boot(run_args(mode='default'))
        self.assertNotIn('--features', build)

    def test_a_tests_boot_refuses_options_it_cannot_honour(self):
        run = load_script('metal-run')
        for changes in [dict(inject='forceoff-fail'), dict(suite='smoke'), dict(disk='install'), dict(payload='hang')]:
            with self.subTest(**changes), patch.object(run, 'host_env', side_effect=AssertionError('refused first')):
                with self.assertRaisesRegex(SystemExit, 'tests mode'):
                    run.boot(run_args(**changes))

    def test_metal_build_compiles_the_testing_kernel_and_delivers_its_test_disk(self):
        build = load_script('metal-build')
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            args = SimpleNamespace(id='20261018T000030Z-metal', root=str(root), repo=str(root / 'repo'), sha='a' * 40,
                                   rust_fork=str(root / 'rust'), cache=str(root / 'cache'), stub=str(root / 'stub'),
                                   guard_target='x86-metal', features='testing', suite='')
            clone, out, tmp = build.paths(args)
            out.mkdir(parents=True)
            image = clone / 'target/release/build/breenix-0/out/breenix-uefi.img'
            stub = tmp / 'stub-target' / build.STUB_TARGET / 'release/metal-efi-stub.efi'
            for path in (image, stub, stub.with_name('metal-boot-manager.efi'), clone / 'testdata/ext2.img', clone / build.TEST_BINARIES):
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(b'BXTEST\0\0' if path.name == 'test_binaries.img' else b'bytes')
            for name in ('docs/boot-path.json', 'xtask/src/boot_stages.rs'):
                (clone / name).parent.mkdir(parents=True, exist_ok=True)
                (clone / name).write_text(name)
            copy_contracts(clone)
            calls = []
            def call(command, **options):
                calls.append(command)
                return 0
            with patch.dict(build.os.environ, CARGO_BUILD_JOBS='4'), \
                 patch.object(build.subprocess, 'run',
                              return_value=SimpleNamespace(returncode=0, stdout=str(root / 'cargo-home') + '\n')), \
                 patch.object(build.subprocess, 'call', side_effect=call):
                self.assertEqual(build.inner(args), 0)
            record = m.read_json(out / 'build.json')
            self.assertEqual((out / 'test-binaries.img').read_bytes(), b'BXTEST\0\0')
            self.assertEqual(record['testBinaries'], {'size': 8, 'sha256': metal_image.sha256(out / 'test-binaries.img')})
            # Main's scoring needs its definition and catalog in the boot record.
            self.assertTrue({'docs/boot-path.json', 'xtask/src/boot_stages.rs'} <= set(record['definition']))
            self.assertTrue(set(CONTRACTS) <= set(record['definition']))
        self.assertEqual(calls[1], ['cargo', 'build', '--release', '--bin', 'qemu-uefi', '--features', 'testing'])
        self.assertEqual((record['profile'], record['features']), ('testing', ['testing']))


class TestsDriverTest(StateTest):
    def driver(self, n=0, mode='tests'):
        args = SimpleNamespace(id=f'20261018T00004{n}Z-metal', mode=mode, payload='image', target='main', task='',
                               suite=None, inject=None, disk='root', input=None, session=False, attended=False,
                               wait=30, nmi=False, commit='a' * 40, ramdisk=True, idle_minutes=30, no_wait=False)
        with patch.object(m, 'load_site', return_value={}):
            d = boot.Driver(args)
        d.dir.mkdir(parents=True)
        d.event, d.phase = Mock(), Mock()
        return d

    def inputs(self, d, build):
        inputs = d.stage / 'inputs'
        inputs.mkdir(parents=True)
        (inputs / 'ready').touch()
        (inputs / 'test-binaries.img').write_bytes(b'BXTEST\0\0')
        (inputs / 'stub.efi').write_bytes(b'stub')
        m.write_json(inputs / 'build.json', dict({'commit': d.args.commit, 'profile': 'testing', 'features': ['testing'],
                                                  'testBinaries': {'size': 8, 'sha256': metal_image.sha256(
                                                      inputs / 'test-binaries.img')}}, **build))
        return inputs

    def test_only_the_testing_build_with_its_recorded_test_disk_is_delivered(self):
        for n, (build, why) in enumerate([({'profile': 'production', 'features': []}, 'provenance'),
                                          ({'testBinaries': {'size': 8, 'sha256': '0' * 64}}, 'test-binary disk')]):
            d = self.driver(n)
            self.inputs(d, build)
            with patch.object(boot.metal_disk, 'stage', side_effect=AssertionError('refused before staging')):
                with self.assertRaisesRegex(boot.Finish, why):
                    d.prepare_payload()
        # A production boot refuses the testing kernel.
        d = self.driver(2, mode='default')
        self.inputs(d, {})
        with self.assertRaisesRegex(boot.Finish, 'provenance'):
            d.prepare_payload()

    def test_the_test_disk_is_the_ram_disk_in_place_of_the_boot_intent(self):
        d = self.driver()
        inputs = self.inputs(d, {})
        assembled = {}
        def assemble(path, files):
            assembled.update(files, ramdisk_bytes=files['ramdisk'].read_bytes())
            Path(path).write_bytes(b'disk')
            return {'size': 4, 'sha256': '0' * 64, 'components': {}}
        with patch.object(boot.metal_image, 'extract_esp', return_value={}), \
             patch.object(boot.metal_disk, 'stage', return_value=({'bytes': 1}, {})) as stage, \
             patch.object(boot.metal_image, 'assemble', side_effect=assemble), \
             patch.object(boot.metal_image, 'stub_efi', return_value={'size': 4, 'sha256': '0' * 64, 'stub': {}}), \
             patch.object(d, 'load_definition'):
            d.prepare_payload()
        self.assertEqual(stage.call_args.kwargs['mode'], 'tests')
        self.assertEqual(assembled['ramdisk_bytes'], b'BXTEST\0\0')
        self.assertEqual(d.delivery['testBinaries']['size'], 8)
        self.assertFalse((inputs / 'test-binaries.img').exists())

    def test_the_boot_ends_at_the_report_with_main_fatal_patterns_and_its_disk_evidence(self):
        d = self.driver()
        definition = d.dir / 'definition'
        (definition / 'docs/metal').mkdir(parents=True)
        m.write_json(definition / 'docs/metal/boot-path.json', {'gateArch': 'x86_64', 'fatal': ['metal only'],
                     'milestones': [{'id': 'idle', 'kernel': True, 'stages': {'x86_64': [{'marker': '[metal] kernel idle'}]}}]})
        m.write_json(definition / 'docs/boot-path.json', {'fatal': ['\\[TEST:[^\\]]*:FAIL'], 'milestones': []})
        copy_contracts(definition)
        d.load_definition()
        self.assertEqual((d.final_marker, d.required_stages), ((boot.TESTS_DONE, 'contains'), []))
        self.assertEqual([p.pattern for p in d.fatal_patterns], ['\\[TEST:[^\\]]*:FAIL'])
        d.t0 = d.kernel_started = 100
        # x86-64 prints the report through the logger (R240 boot 20261019T012239Z-metal).
        report = REPORT
        d.on_line('[ INFO] kernel::userspace_test: ' + LOADER)
        d.on_line('[ INFO] kernel::syscall::handlers: ' + TALLY)
        d.on_line('[ INFO] kernel::syscall::handlers: USERSPACE TEST REPORT')
        self.assertFalse(d.final_seen)
        d.on_line(report)
        self.assertFalse(d.completion_reported)  # the installed root is not yet confirmed
        d.seen.add('disk-root')
        with self.assertRaises(boot.Finish) as done:
            d.on_line(report)
        self.assertEqual(done.exception.status, m.EXIT_DONE)
        d = self.driver(1)
        copy_contracts(d.dir / 'definition')
        d.t0 = d.kernel_started = 100
        d.final_marker = (boot.TESTS_DONE, 'contains')
        d.seen.add('disk-root')
        for line in (LOADER, 'TCP_DUP_LISTENER_TEST_FAILED',
                     'TEST_TALLY: exited=2 nonzero=1 failed=[tcp_dup_listener_test:1] started=2'):
            d.on_line(line)
        with self.assertRaises(boot.Finish) as failed:
            d.on_line(REPORT)
        self.assertEqual(failed.exception.status, m.EXIT_SHORT)
        self.assertIn('canonical test failed', str(failed.exception))
        d = self.driver(2)
        d.fatal_patterns, d.t0, d.kernel_started = [re.compile('\\[TEST:[^\\]]*:FAIL')], 100, 100
        d.on_line('[TEST:fork:FAIL]')
        self.assertIsNotNone(d.fatal_at)


class TestsScoreTest(StateTest):
    def test_scores_main_path_milestones_without_announcements_and_names_missing_stages(self):
        directory = m.STATE / 'boot'
        definition = directory / 'definition'
        (definition / 'docs').mkdir(parents=True)
        copy_contracts(definition)
        m.write_json(definition / 'docs/boot-path.json', {'milestones': [
            {'id': 'arch-bringup', 'kernel': True, 'stages': {'aarch64': ['ARM only'], 'x86_64': ['Kernel entry', 'GDT']}},
            {'id': 'boot-logo', 'stub': True, 'stages': {'x86_64': [{'name': 'Logo', 'marker': 'logo drawn'}]}},
            {'id': 'process-lifecycle', 'stages': {'x86_64': ['Fork', 'Exec'], 'probe': ['Fork']}},
            {'id': 'aarch64-only', 'stages': {'aarch64': ['ARM only']}}]})
        catalog = definition / 'tools/breenix-runs/Sources/BreenixRuns/Resources/boot-stages-x86_64.json'
        catalog.parent.mkdir(parents=True)
        m.write_json(catalog, {'stages': [{'name': 'Kernel entry', 'marker': 'Kernel entry point reached'},
                                          {'name': 'GDT', 'marker': 'GDT initialized|GDT ready'},
                                          {'name': 'Fork', 'marker': 'FORK_TEST_PASSED'},
                                          {'name': 'Exec', 'marker': 'EXEC_TEST_PASSED'}]})
        m.write_json(directory / 'run.json', {'mode': 'tests', 'disk': 'root', 'kind': 'boot'})
        (directory / 'events.jsonl').write_text('{"event": "power-on"}\n')
        # Out of order still counts; the runner's announcement of a marker does not.
        (directory / 'serial.txt').write_text('\n'.join([
            'GDT ready', '[ INFO] kernel::test_exec:     -> Userspace will emit EXEC_TEST_PASSED marker if successful',
            'Kernel entry point reached', 'FORK_TEST_PASSED', 'logo drawn']) + '\n')
        verdict = metal_score.score(directory)
        self.assertEqual(verdict, {'bootShape': 'disk-root', 'definition': 'docs/boot-path.json', 'captureComplete': True,
                                   'testReport': {'passed': False, 'reason': 'userspace test report is incomplete'},
                                   'milestones': {
                                       'arch-bringup': {'reached': 2, 'total': 2, 'passed': True, 'missing': []},
                                       'process-lifecycle': {'reached': 1, 'total': 2, 'passed': False,
                                                             'missing': ['Exec']}}})
        self.assertTrue(metal_score.falls_short(verdict))
        markers = 'GDT ready\nKernel entry point reached\nFORK_TEST_PASSED\nEXEC_TEST_PASSED\n'
        good = '\n'.join((LOADER, TALLY, REPORT)) + '\n'
        bad_reports = [
            (good.replace(TALLY, 'TCP_DUP_LISTENER_TEST_FAILED\nTEST_TALLY: exited=2 nonzero=1 '
                                 'failed=[tcp_dup_listener_test:1] started=2'), 'canonical test failed'),
            (good.replace(LOADER, ''), 'loader summary'),
            (good.replace(LOADER, LOADER + '\n' + LOADER), 'loader summary'),
            (good.replace(LOADER, f'[test] Loaded {COUNT-1}/{COUNT} test binaries (0 failed, 1 not found)'),
             'unloaded test programs'),
            ('READONLY_EXT2: SKIP (no fixture attached)\n' + good, 'skipped test'),
            (good.replace(TALLY, ''), 'no completed exit accounting'),
            (good.replace('started=2', 'started=3'), 'only 2 exited'),
            (good.replace(TALLY, 'TEST_TALLY: exited=2 nonzero=1 failed=[unknown_child:1] started=2'),
             'unexpected process exit'),
            (good.replace(TALLY, 'TEST_TALLY: exited=2 nonzero=1 failed=[] started=2'), 'named failures'),
        ]
        for report, reason in bad_reports:
            with self.subTest(reason=reason):
                (directory / 'serial.txt').write_text(markers + report)
                verdict = metal_score.score(directory)
                self.assertTrue(all(item['passed'] for item in verdict['milestones'].values()))
                self.assertTrue(metal_score.falls_short(verdict))
                self.assertIn(reason, verdict['testReport']['reason'])
        for report in (good, good.replace(TALLY, 'TEST_TALLY: exited=2 nonzero=1 '
                                               'failed=[sigkill_teardown_test_child_18:-9] started=2')):
            (directory / 'serial.txt').write_text(markers + report)
            self.assertFalse(metal_score.falls_short(metal_score.score(directory)))


if __name__ == '__main__':
    unittest.main()
