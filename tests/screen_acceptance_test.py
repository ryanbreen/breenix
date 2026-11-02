"""Regression checks for the incomplete PR #2642 acceptance report."""
import json
from pathlib import Path
import runpy
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
HEAD = 'a' * 40


class ScreenAcceptance(unittest.TestCase):
    def setUp(self):
        self.api = runpy.run_path(str(ROOT / 'scripts/screen-proof-acceptance.py'))

    def test_truncated_screen_records_are_reported_as_invalid(self):
        for suffix in ('', 'draw/one', 'draw/one end'):
            with self.subTest(suffix=suffix):
                with self.assertRaisesRegex(ValueError, 'malformed SCREEN'):
                    self.api['screen_inventory']('gpu', 'SUITE gpu SCREEN ' + suffix)

    def test_cross_target_pictures_and_unobservable_are_distinct(self):
        digest = 'a' * 64
        native = (f'SUITE gpu SCREEN profile/example end sha256={digest} mode=2x1 binding=2x1\n'
                  'SUITE gpu SCREEN readback/lost end unobservable reason=target-invalidated errno=5 eio_reason=1 failed_submission=1\n')
        arm = native.replace(' unobservable reason=target-invalidated errno=5 eio_reason=1 failed_submission=1',
                             f' sha256={digest} mode=2x1 binding=2x1')
        pictures, declarations = self.api['screen_inventory']('gpu', native)
        self.assertEqual(len(pictures), 1)
        self.assertEqual(len(declarations), 1)
        self.assertNotIn(('readback/lost', 'end'), pictures)
        self.assertEqual(self.api['compare_pictures'](pictures, self.api['screen_inventory']('gpu', arm)[0]), 1)
        with self.assertRaisesRegex(ValueError, 'digest'):
            self.api['compare_pictures'](pictures, self.api['screen_inventory']('gpu', arm.replace(digest, 'b' * 64, 1))[0])

    def test_native_pair_cannot_prove_tandem_acceptance(self):
        failures = self.api['missing_captures']({'console': Path('c'), 'gpu': Path('g')})
        self.assertEqual(failures, ['missing arm-gpu capture', 'missing arm-breengel capture'])

    def test_cross_target_requires_a_compared_picture(self):
        digest = 'a' * 64
        native = {('draw/one', 'end'): digest}
        arm = {('draw/two', 'end'): digest}
        for left, right in (({}, {}), ({}, arm), (native, {}), (native, arm)):
            with self.subTest(native=left, arm=right):
                with self.assertRaisesRegex(ValueError, 'no cross-target pictures compared'):
                    self.api['compare_pictures'](left, right)

    def test_stale_arm_and_mismatched_native_tools_are_rejected(self):
        with tempfile.TemporaryDirectory() as tmp:
            p = Path(tmp)
            boot = dict(id='fixture', platform='qemu', arch='aarch64', mode='suite', suite='gpu')
            for name, text in [('commit', HEAD), ('dirty', '0'), ('ended', '1'), ('exit-status', '0')]:
                (p / name).write_text(text)
            (p / 'run.json').write_text(json.dumps(boot))
            self.api['capture_identity'](p, HEAD, 'gpu', 'qemu')
            with self.assertRaisesRegex(ValueError, 'commit'):
                self.api['capture_identity'](p, 'b' * 40, 'gpu', 'qemu')
            boot.update(platform='metal', arch='x86_64', commit=HEAD, toolsCommit='b' * 40,
                        kind='boot', evidenceEligible=True,
                        build=dict(commit=HEAD, profile='production', features=[]))
            (p / 'metal-run.json').write_text(json.dumps(boot))
            with self.assertRaisesRegex(ValueError, 'tools'):
                self.api['capture_identity'](p, HEAD, 'gpu', 'metal')

    def test_overlap_is_counted_once_and_keeps_both_boots(self):
        rows = self.api['check_inventory']({
            'console-boot': [{'id': 'graphics/channel', 'result': 'PASS'},
                             {'id': 'recovery/fault-reset', 'result': 'PASS'}],
            'gpu-boot': [{'id': 'graphics/channel', 'result': 'PASS'}],
        })
        self.assertEqual(len(rows), 2)
        self.assertEqual(rows['graphics/channel'], ['console-boot', 'gpu-boot'])
        with self.assertRaisesRegex(ValueError, 'failed'):
            self.api['check_inventory']({'gpu': [{'id': 'graphics/channel', 'result': 'FAIL'}]})

    def test_reset_requires_its_own_pending_case_and_reset_witness(self):
        names = ['submit/device-reset-recovers', 'textures/upload-visibility', 'isolation/killable-wait']
        lines = []
        for name in names:
            lines += [f'SUITE gpu WAIT {name} until=5000 for=4000 result=reset-ms',
                      'GT710_NATIVE_FAULT reason=shared core requested reset instance=0x1000 epoch=1 put=2 registers=[]',
                      f'SUITE gpu VALUE {name} reset-ms=4002ms',
                      f'SUITE gpu CASE {name} PASS ms=5000']
        serial = '\n'.join(lines) + '\n'
        rows = self.api['reset_faults'](serial, names)
        self.assertEqual([r['case'] for r in rows], names)
        for row in rows:
            self.assertIn('WITHHOLD_ALL', row['cause'])
            self.assertIn('source', row)
        for bad in [serial.replace('shared core requested reset', 'GR trap', 1),
                    serial.replace('VALUE textures/upload-visibility reset-ms=',
                                   'VALUE textures/copy-region reset-ms='),
                    serial.replace('CASE isolation/killable-wait PASS', 'CASE isolation/killable-wait FAIL'),
                    'GT710_NATIVE_FAULT reason=shared core requested reset\n' + serial]:
            with self.assertRaises(ValueError):
                self.api['reset_faults'](bad, names)

    def test_exported_metal_metadata_uses_full_run_json(self):
        with tempfile.TemporaryDirectory() as tmp:
            p = Path(tmp)
            boot = dict(id='fixture-metal', platform='metal', arch='x86_64', mode='suite',
                        suite='gpu', commit=HEAD, toolsCommit=HEAD, kind='boot',
                        evidenceEligible=True,
                        build=dict(commit=HEAD, profile='production', features=[]))
            (p / 'run.json').write_text(json.dumps(boot))
            for name, text in [('commit', HEAD), ('dirty', '0'), ('ended', '1'), ('exit-status', '0')]:
                (p / name).write_text(text)
            self.assertEqual(self.api['capture_identity'](p, HEAD, 'gpu', 'metal')['toolsCommit'], HEAD)
            # Falling back to run.json must still verify its actual tools commit.
            boot['toolsCommit'] = 'b' * 40
            (p / 'run.json').write_text(json.dumps(boot))
            with self.assertRaisesRegex(ValueError, 'tools'):
                self.api['capture_identity'](p, HEAD, 'gpu', 'metal')

    def test_reset_witness_may_precede_fault_only_in_its_pending_case(self):
        names = ['submit/watchdog-quarantine', *self.api['RESET_SOURCES']]
        previous = f'SUITE gpu CASE {names[0]} PASS ms=2107'
        value = f'SUITE gpu VALUE {names[1]} reset-ms=4001ms expect=4000..4850ms'
        fault = ('GT710_NATIVE_FAULT reason=shared core requested reset '
                 'instance=0x1000 epoch=1 put=2 registers=[]')
        lines = [previous]
        for name in names[1:]:
            lines += [f'SUITE gpu WAIT {name} until=5000 for=4000 result=reset-ms',
                      value if name == names[1] else f'SUITE gpu VALUE {name} reset-ms=4627ms',
                      fault, f'SUITE gpu CASE {name} PASS ms=5107']
        serial = '\n'.join(lines) + '\n'
        rows = self.api['reset_faults'](serial, names)
        self.assertEqual([row['case'] for row in rows], names[1:])
        self.assertIn(value, rows[0]['witnesses'])
        for bad in [serial.replace(value + '\n', '', 1),
                    serial.replace(value + '\n', '', 1).replace(previous, value + '\n' + previous, 1),
                    serial.replace(fault, fault + '\n' + value, 1)]:
            with self.subTest(serial=bad):
                with self.assertRaisesRegex(ValueError, 'reset-ms witness'):
                    self.api['reset_faults'](bad, names)

    def test_events_are_required_only_for_metal(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            for name, platform, arch in [('arm-gpu', 'qemu', 'aarch64'),
                                         ('gpu', 'metal', 'x86_64')]:
                p = root / name
                p.mkdir()
                boot = dict(id=name, platform=platform, arch=arch, mode='suite', suite='gpu',
                            commit=HEAD, toolsCommit=HEAD, kind='boot', evidenceEligible=True,
                            build=dict(commit=HEAD, profile='production', features=[]))
                (p / 'run.json').write_text(json.dumps(boot))
                for filename, value in [('commit', HEAD), ('dirty', '0'), ('ended', '1'),
                                        ('exit-status', '0'), ('serial.txt', '')]:
                    (p / filename).write_text(value)
            report = self.api['accept'](HEAD, {'arm-gpu': root / 'arm-gpu', 'gpu': root / 'gpu'})
            self.assertNotIn('events.jsonl', report['captures']['arm-gpu']['failure'])
            self.assertIn('events.jsonl', report['captures']['gpu']['failure'])

    def test_native_raster_join_uses_the_edid_dimensions(self):
        fixture = runpy.run_path(str(ROOT / 'tests/gt710_display_evidence_test.py'))
        native = dict(fixture['API'])
        mode = dict(width=1280, height=1024, clock_khz=108000)
        native['board_inventory'] = lambda serial: dict(modes=[mode])
        serial = fixture['text']([fixture['row'](0), fixture['row'](1)])
        self.api['check_runtime_display'](serial, native)
        mode['height'] = 800
        with self.assertRaisesRegex(ValueError, 'runtime raster differs'):
            self.api['check_runtime_display'](serial, native)


if __name__ == '__main__':
    unittest.main()
