#!/usr/bin/env python3
"""Recovery boot classification, staging and scoring; no hardware needed."""
from metal_tools_test import StateTest, boot, load_script, m
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch
import metal_score


class FaultTest(StateTest):
    def args(self, **changes):
        values = dict(id='20261018T000001Z-metal', mode='console', payload='image',
                      target='branch:fixture', task='', suite=None, inject='xhci-short-read',
                      disk=None, input='sol', session=False, attended=False, wait=30, nmi=False,
                      commit='a' * 40)
        return SimpleNamespace(**dict(values, **changes))

    def test_fault_boot_is_ineligible_and_uses_its_own_terminal_stage(self):
        for inject, name in [('xhci-reset-device', 'reset-device'), ('xhci-short-read', 'short-read')]:
            args = self.args(inject=inject, id='20261018T00000' + str(1 if name == 'reset-device' else 2) + 'Z-metal')
            with patch.object(m, 'load_site', return_value={}):
                d = boot.Driver(args)
            d.create_record()
            record = m.read_json(d.dir / 'run.json')
            self.assertEqual(record['kind'], 'fault-test')
            self.assertFalse(record['evidenceEligible'])
            with self.assertRaisesRegex(ValueError, 'fault tests'):
                metal_score.score(d.dir)
            d.load_definition()
            self.assertEqual(d.required_stages, [[('[xhci-fault] ' + name + ' PASS:', 'contains')]])
            self.assertTrue(d.console_done)
            with patch.object(d, 'console_send', side_effect=AssertionError('must not type')):
                d.challenge_console('Breenix login: ')
            d.t0 = 1
            with self.assertRaises(boot.Finish) as result:
                d.on_line('[xhci-fault] ' + name + ' PASS: input-report-decoded-and-requeued')
            self.assertEqual(result.exception.status, m.EXIT_DONE)
            self.assertFalse(any(d.required_stages))

    def test_rejects_incompatible_fault_options(self):
        for changes in [dict(mode='probe'), dict(session=True), dict(attended=True),
                        dict(disk='root'), dict(payload='none'), dict(input=None)]:
            with self.assertRaises(ValueError):
                boot.validate_xhci_fault(self.args(**changes))
        boot.validate_xhci_fault(self.args())

    def test_only_a_fault_boot_takes_a_fault_test_build(self):
        cases = [('xhci-short-read', 'production', [], 'provenance'),
                 (None, 'fault-test', ['xhci_fault_inject'], 'provenance'),
                 ('xhci-short-read', 'fault-test', ['xhci_fault_inject'], 'staged')]
        for n, (inject, profile, features, outcome) in enumerate(cases):
            args = self.args(inject=inject, id=f'20261018T00001{n}Z-metal')
            with patch.object(m, 'load_site', return_value={}):
                d = boot.Driver(args)
            d.dir.mkdir(parents=True)
            d.event, d.phase = Mock(), Mock()
            inputs = d.stage / 'inputs'
            inputs.mkdir(parents=True)
            (inputs / 'ready').touch()
            m.write_json(inputs / 'build.json', {'commit': args.commit, 'profile': profile, 'features': features})
            with patch.object(boot.metal_root, 'stage_root', side_effect=RuntimeError('staged')):
                with self.assertRaises((boot.Finish, RuntimeError)) as raised:
                    d.prepare_payload()
            self.assertIn(outcome, str(raised.exception))

    def test_scorer_refuses_a_fault_even_with_old_misclassified_record(self):
        directory = m.STATE / 'legacy'
        directory.mkdir()
        m.write_json(directory / 'run.json', {'kind': 'boot', 'inject': 'xhci-short-read'})
        with self.assertRaisesRegex(ValueError, 'fault tests'):
            metal_score.score(directory)


class FaultBuildTest(unittest.TestCase):
    def test_only_xhci_fault_boots_request_the_fault_test_kernel(self):
        run = load_script('metal-run')
        for inject, features in [('xhci-reset-device', ['--features', 'xhci_fault_inject']),
                                 ('xhci-short-read', ['--features', 'xhci_fault_inject']), (None, [])]:
            args = SimpleNamespace(session=False, attended=False, wait=30, idle_minutes=30, mode='console',
                                   input='sol', payload='image', disk=None, inject=inject, sha=None,
                                   branch='fixture', suite=None, no_wait=False, nmi=False, linux_directory=None,
                                   serve_as=None, content_type=None, ramdisk=True)
            with tempfile.TemporaryDirectory() as home, \
                 patch.object(run, 'host_env', return_value={}), \
                 patch.object(run, 'resolve', return_value='a' * 40), \
                 patch.object(run, 'in_metal', return_value=SimpleNamespace(returncode=0, stdout='', stderr='')), \
                 patch.object(run.Path, 'home', return_value=Path(home)), \
                 patch.object(run.subprocess, 'Popen') as popen:
                self.assertEqual(run.boot(args), 0)
            self.assertEqual(popen.call_args.args[0][2:], ['_build', popen.call_args.args[0][3], 'a' * 40] + features)

    def test_metal_build_compiles_and_records_the_fault_test_kernel(self):
        build = load_script('metal-build')
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            args = SimpleNamespace(id='20261018T000020Z-metal', root=str(root), repo=str(root / 'repo'), sha='a' * 40,
                                   rust_fork=str(root / 'rust'), cache=str(root / 'cache'), stub=str(root / 'stub'),
                                   guard_target='x86-metal', features='xhci_fault_inject', suite='')
            clone, out, tmp = build.paths(args)
            out.mkdir(parents=True)
            image = clone / 'target/release/build/breenix-0/out/breenix-uefi.img'
            stub = tmp / 'stub-target' / build.STUB_TARGET / 'release/metal-efi-stub.efi'
            for path in (image, stub, stub.with_name('metal-boot-manager.efi'), clone / 'testdata/ext2.img'):
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(b'bytes')
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
        self.assertEqual(calls[1], ['cargo', 'build', '--release', '--bin', 'qemu-uefi',
                                    '--features', 'xhci_fault_inject'])
        self.assertEqual((record['profile'], record['features']), ('fault-test', ['xhci_fault_inject']))


if __name__ == '__main__':
    unittest.main()
