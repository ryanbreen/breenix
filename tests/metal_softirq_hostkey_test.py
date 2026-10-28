#!/usr/bin/env python3
"""The forcing boot must deliver its own kernel profile and require its PASS."""
from metal_tools_test import StateTest, boot, load_script, m
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch


def args(**changes):
    return SimpleNamespace(**dict(dict(
        id='20261027T000001Z-metal', mode='suite', payload='image', target='branch:fixture',
        task='', suite='console', inject='softirq-hostkey', disk='root', input=None,
        session=False, wait=30, idle_minutes=30, nmi=False, commit='a' * 40, sha=None,
        branch='fixture', no_wait=False, linux_directory=None, serve_as=None,
        content_type=None, ramdisk=True), **changes))


class RunnerTest(unittest.TestCase):
    def test_forcing_suite_selects_only_its_feature(self):
        run = load_script('metal-run')
        with tempfile.TemporaryDirectory() as home, \
             patch.object(run, 'host_env', return_value={}), \
             patch.object(run, 'resolve', return_value='a' * 40), \
             patch.object(run, 'resolve_suites', return_value='console'), \
             patch.object(run, 'in_metal', return_value=SimpleNamespace(returncode=0, stdout='', stderr='')), \
             patch.object(run.Path, 'home', return_value=Path(home)), \
             patch.object(run.subprocess, 'Popen') as popen:
            self.assertEqual(run.boot(args()), 0)
        command = popen.call_args_list[0].args[0]
        self.assertEqual(command[command.index('--features') + 1], 'softirq_hostkey_test')
        self.assertEqual(command[command.index('--suite') + 1], 'console')
        self.assertEqual(load_script('metal-build').PROFILES['softirq_hostkey_test'], 'fault-test')


class DriverTest(StateTest):
    def driver(self, **changes):
        with patch.object(m, 'load_site', return_value={}):
            d = boot.Driver(args(**changes))
        d.create_record()
        d.event, d.phase = Mock(), Mock()
        return d

    def test_forcing_pass_is_required_and_not_milestone_evidence(self):
        d = self.driver()
        record = m.read_json(d.dir / 'run.json')
        self.assertEqual(record['kind'], 'fault-test')
        self.assertFalse(record['evidenceEligible'])
        d.load_definition()
        self.assertIn([('SOFTIRQ_HOSTKEY PASS', 'exact')], d.required_stages)

    def test_profile_mismatches_are_rejected_before_disk_staging(self):
        for n, (inject, profile, features, outcome) in enumerate([
            ('softirq-hostkey', 'production', [], 'provenance'),
            (None, 'fault-test', ['softirq_hostkey_test'], 'provenance'),
            ('softirq-hostkey', 'fault-test', ['softirq_hostkey_test'], 'staged'),
        ]):
            d = self.driver(id=f'20261027T00001{n}Z-metal', inject=inject)
            inputs = d.stage / 'inputs'
            inputs.mkdir(parents=True)
            (inputs / 'ready').touch()
            m.write_json(inputs / 'build.json', dict(commit=d.args.commit, profile=profile,
                                                    features=features, suite='console'))
            m.write_json(inputs / 'definition/docs/suites/console.json', {'id': 'console'})
            with patch.object(boot.metal_image, 'extract_esp', return_value={}), \
                 patch.object(boot.metal_disk, 'stage', side_effect=RuntimeError('staged')):
                with self.assertRaises((boot.Finish, RuntimeError)) as raised:
                    d.prepare_payload()
            self.assertIn(outcome, str(raised.exception))

    def test_forcing_marker_consumes_only_the_complete_pass_line(self):
        d = self.driver()
        d.load_definition()
        d.t0 = 1
        d.on_line('Initializing PCIe, USB, and Video... Done')
        d.on_line('announced SOFTIRQ_HOSTKEY PASS')
        self.assertTrue(any(d.required_stages))
        d.on_line('SOFTIRQ_HOSTKEY PASS')
        self.assertFalse(any(d.required_stages))


if __name__ == '__main__':
    unittest.main()
