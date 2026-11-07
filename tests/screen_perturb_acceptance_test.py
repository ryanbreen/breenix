"""E22 u10: phase evidence and the U4 native-only acceptance profile."""
import copy
import hashlib
import json
from pathlib import Path
import runpy
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
HEAD = 'a' * 40
SHA = 'a' * 64
API = runpy.run_path(str(ROOT / 'scripts/screen-proof-acceptance.py'))


def config(changed=False):
    return dict(mode='2x1', raster='1280x1024', viewport=[0, 112, 1280, 800],
                out='sor0', sources=['sor0', 'rg0'], methods=[[0x4a0, int(changed)]])


def digest(value):
    return hashlib.sha256(json.dumps(value, sort_keys=True, separators=(',', ':')).encode()).hexdigest()


def fixture(suite='gpu'):
    case = 'draw/one'
    entry = dict(suite=suite, case=case, checkpoint='end', sha256=SHA, mode='2x1',
                 native=[dict(config=digest(config()), out='sor0', raster='1280x1024',
                              crc='12345678', crc2='abcdef12')])
    rows, lines = [], []
    for phase, word in enumerate(('one', 'two', 'three', 'four'), 1):
        changed = phase in (2, 3)
        crc = '87654321' if changed else '12345678'
        rows.append(dict(suite=suite, case=case, checkpoint='perturb-' + word, backend=6,
            binding_before=dict(resource=7, present_fence=9), binding_after=dict(resource=7, present_fence=9),
            elapsed_us=85000, disarmed=True, display_error=0, status=1,
            output_config=config(changed), samples=[dict(status=1, crc=crc, crc2='abcdef12')] * 5))
        lines.append(f'SUITE {suite} SCREENPERTURB {case} end phase={phase} sha256={SHA} '
                     f'mode=2x1 binding=2x1 crc={crc} crc2=abcdef12 raster=1280x1024 out=sor0 '
                     f'resource=7 present_fence=9 fired={int(phase > 1)} disarmed={int(not changed)}')
    screen = (f'SUITE {suite} SCREEN {case} end sha256={SHA} mode=2x1 binding=2x1 '
              'crc=12345678 crc2=abcdef12 raster=1280x1024 out=sor0')
    return '\n'.join([screen, *lines]) + '\n', dict(version=1, entries=[entry]), rows


class PerturbationAcceptance(unittest.TestCase):
    def check(self, serial=None, table=None, rows=None):
        text, approved, records = fixture()
        return API['check_perturbation']('gpu', text if serial is None else serial,
            approved if table is None else table, records if rows is None else rows)

    def test_four_phases_and_native_baseline(self):
        self.assertEqual(self.check()['phases'], 4)
        text, table, rows = fixture()
        lines = text.splitlines()
        for phase in range(1, 5):
            with self.subTest(missing=phase), self.assertRaises(ValueError):
                self.check('\n'.join(l for l in lines if f'phase={phase} ' not in l))
            with self.subTest(repeated=phase), self.assertRaises(ValueError):
                self.check(text + lines[phase] + '\n')
        for order in ([2, 1, 3, 4], [1, 3, 2, 4], [1, 2, 4, 3]):
            with self.subTest(order=order), self.assertRaises(ValueError):
                self.check('\n'.join([lines[0], *(lines[i] for i in order)]))
        for field, value in [('sha256', 'b' * 64), ('native', []), ('mode', '3x1')]:
            bad = copy.deepcopy(table); bad['entries'][0][field] = value
            with self.subTest(approval=field), self.assertRaises(ValueError):
                self.check(table=bad)
        for phase in range(4):
            bad = copy.deepcopy(rows); bad.pop(phase)
            with self.subTest(record=phase), self.assertRaises(ValueError):
                self.check(rows=bad)

    def test_negative_digest_crc_fired_binding_and_restoration(self):
        text, _, rows = fixture()
        mutations = [text.replace('phase=3 sha256=' + SHA, 'phase=3 sha256=' + 'b' * 64),
                     text.replace('phase=3 ', 'phase=3 ', 1).replace('crc=87654321', 'crc=12345678'),
                     text.replace('phase=4 ', 'phase=4 ', 1).replace('disarmed=1\n', 'disarmed=0\n'),
                     text.replace('fired=1', 'fired=0'), text.replace('resource=7', 'resource=8'),
                     text.replace('SCREENPERTURB draw/one end', 'SCREENPERTURB draw/two end', 1)]
        for bad in mutations:
            with self.subTest(serial=bad), self.assertRaises(ValueError):
                self.check(bad)
        for phase in range(4):
            for field, value in [('disarmed', False), ('display_error', 1), ('status', 3), ('elapsed_us', 250001)]:
                bad = copy.deepcopy(rows); bad[phase][field] = value
                with self.subTest(phase=phase, field=field), self.assertRaises(ValueError):
                    self.check(rows=bad)
        bad = copy.deepcopy(rows); bad[2]['samples'] = rows[0]['samples']
        unchanged = text.replace('phase=3 sha256=' + SHA + ' mode=2x1 binding=2x1 crc=87654321',
                                 'phase=3 sha256=' + SHA + ' mode=2x1 binding=2x1 crc=12345678')
        with self.assertRaises(ValueError): self.check(unchanged, rows=bad)
        bad = copy.deepcopy(rows); bad[3]['output_config'] = config(True)
        with self.assertRaises(ValueError): self.check(rows=bad)
        bad = copy.deepcopy(rows); bad[3]['samples'] = rows[2]['samples']
        unrestored = text.replace('phase=4 sha256=' + SHA + ' mode=2x1 binding=2x1 crc=12345678',
                                  'phase=4 sha256=' + SHA + ' mode=2x1 binding=2x1 crc=87654321')
        with self.assertRaises(ValueError): self.check(unrestored, rows=bad)

    def test_r240_requirements_and_arm_profile(self):
        captures = {name: Path(name) for name in ('console', 'gpu', 'breengel')}
        self.assertEqual(API['missing_captures'](captures, profile='r240'), [])
        self.assertEqual(API['missing_captures'](captures),
                         ['missing arm-gpu capture', 'missing arm-breengel capture'])
        for missing in captures:
            self.assertEqual(API['missing_captures']({k: v for k, v in captures.items() if k != missing},
                profile='r240'), [f'missing {missing} capture'])
        report = API['accept'](HEAD, {}, profile='r240')
        self.assertFalse(report['proven'])
        self.assertEqual(report['profile'], 'r240')
        self.assertFalse(any('arm-' in failure for failure in report['failures']))

    def test_r240_screen_gate_is_strict_native_and_proof(self):
        text, table, rows = fixture()
        manifest = dict(id='gpu', categories=[dict(id='draw', cases=[dict(id='one')])])
        text = 'SUITE gpu START cases=1 screen_proof=on\nSUITE gpu CASE draw/one PASS ms=1\n' + text
        run = dict(screenProof=True, id='fixture', commit=HEAD)
        baseline = [dict(rows[0], checkpoint='end')]
        self.assertEqual(API['r240_screens'](manifest, text, table, run, baseline, rows)[0], 1)
        for change in ('production', 'extra', 'missing-native', 'crc', 'unapproved', 'missing-end'):
            bad = copy.deepcopy(table); serial = text; boot = dict(run)
            if change == 'production':
                serial = text.replace(' screen_proof=on', ''); boot['screenProof'] = False
            elif change == 'extra': serial += text.split('SUITE gpu SCREEN ', 1)[1].splitlines()[0].join(['SUITE gpu SCREEN ', '\n']).replace('draw/one end', 'draw/one other')
            elif change == 'missing-native': bad['entries'][0]['native'] = []
            elif change == 'crc': serial = text.replace('crc=12345678', 'crc=11111111')
            elif change == 'unapproved': bad['entries'] = []
            elif change == 'missing-end': serial = '\n'.join(l for l in text.splitlines() if ' SCREEN ' not in l) + '\n'
            with self.subTest(change=change), self.assertRaises(ValueError):
                API['r240_screens'](manifest, serial, bad, boot, baseline, rows)

    def test_native_health_requires_memory_and_full_clocks(self):
        text = ('cmdline gt710.clocks=full\n'
                'SUITE gpu MEMORY before free=1000 heap_free_kb=500 user_pages=100\n'
                'SUITE gpu MEMORY after free=1000 heap_free_kb=500 user_pages=100\n'
                'SUITE gpu MEMORY reclaimed=PASS free_before=1000 free_after=1000 allowance=48\n')
        API['r240_health']('gpu', text)
        for bad in (text.replace('full', 'post'), text.replace('gt710.clocks=full', ''),
                    text.replace('reclaimed=PASS', 'reclaimed=FAIL'),
                    text.replace('allowance=48', 'allowance=64'),
                    text.replace('free_after=1000', 'free_after=900')):
            with self.subTest(serial=bad), self.assertRaises(ValueError):
                API['r240_health']('gpu', bad)

    def test_native_companions_bind_kernel_and_tools_to_one_commit(self):
        with tempfile.TemporaryDirectory() as tmp:
            p = Path(tmp)
            for key, value in [('commit', HEAD), ('dirty', '0'), ('exit-status', '0'), ('ended', '1')]:
                (p / key).write_text(value)
            boot = dict(id='native', suite='breengel', kind='boot', mode='suite', platform='metal',
                arch='x86_64', toolsCommit=HEAD, commit=HEAD, evidenceEligible=True,
                build=dict(commit=HEAD, profile='production', features=[]), screenProof=True)
            (p / 'run.json').write_text(json.dumps(boot))
            API['capture_identity'](p, HEAD, 'breengel', 'metal')
            for field in ('commit', 'toolsCommit'):
                bad = dict(boot, **{field: 'b' * 40})
                (p / 'run.json').write_text(json.dumps(bad))
                with self.subTest(field=field), self.assertRaises(ValueError):
                    API['capture_identity'](p, HEAD, 'breengel', 'metal')


if __name__ == '__main__':
    unittest.main()
