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

    def test_native_pair_cannot_prove_tandem_acceptance(self):
        failures = self.api['missing_captures']({'console': Path('c'), 'gpu': Path('g')})
        self.assertEqual(failures, ['missing arm-gpu capture', 'missing arm-breengel capture'])

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


if __name__ == '__main__':
    unittest.main()
