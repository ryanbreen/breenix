"""E22 u10: phase evidence and the U4 native-only acceptance profile."""
import copy
import hashlib
import json
from pathlib import Path
import runpy
import tempfile
import struct
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
        text = ('GT710_CLOCK_STARTUP full=true status=ok\n'
                'SUITE gpu MEMORY before free=1000 heap_free_kb=500 user_pages=100\n'
                'SUITE gpu MEMORY after free=1000 heap_free_kb=500 user_pages=100\n'
                'SUITE gpu MEMORY reclaimed=PASS free_before=1000 free_after=1000 allowance=48\n')
        API['r240_health']('gpu', text)
        for bad in (text.replace('full=true', 'full=false'), text.replace('GT710_CLOCK_STARTUP full=true status=ok', ''),
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

    def test_r240_health_reads_producer_full_clock_evidence(self):
        text = ('GT710_CLOCK_STARTUP full=true gpc_khz=1306451 gpc_counter_khz=81651 '
                'memory_khz=900001 temperature_c=32 status=ok\n'
                'SUITE gpu MEMORY before free=1000 heap_free_kb=500 user_pages=100\n'
                'SUITE gpu MEMORY after free=1000 heap_free_kb=500 user_pages=100\n'
                'SUITE gpu MEMORY reclaimed=PASS free_before=1000 free_after=1000 allowance=48\n')
        API['r240_health']('gpu', text)
        for bad in (text.replace('full=true', 'full=false'),
                    text.replace('status=ok', 'status=failed'),
                    text + 'GT710_CLOCK_FALLBACK reason=failed\n',
                    text + 'GT710_CLOCK_GUARD reason=failed\n',
                    text + 'GT710_CLOCK_CRITICAL reason=failed\n',
                    text + 'cmdline gt710.clocks=post\n'):
            with self.subTest(bad=bad), self.assertRaises(ValueError):
                API['r240_health']('gpu', bad)

    def test_shared_verdict_admits_phases_without_changing_case_counts(self):
        scorer = runpy.run_path(str(ROOT / 'scripts/suite-verdict.py'))
        text, _, _ = fixture()
        text = ('SUITE gpu START cases=1 screen_proof=on\n'
                'SUITE gpu MEMORY before free=1000 heap_free_kb=500 user_pages=100\n'
                'SUITE gpu CASE draw/one PASS ms=1\n' + text +
                'SUITE gpu DONE passed=1 failed=0 skipped=0 total=1\n'
                'SUITE gpu MEMORY after free=1000 heap_free_kb=500 user_pages=100\n'
                'SUITE gpu MEMORY reclaimed=PASS free_before=1000 free_after=1000 allowance=48\n')
        manifest = dict(id='gpu', categories=[dict(id='draw', cases=[dict(id='one')])])
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / 'serial.txt'; path.write_text(text)
            ok, why = scorer['verdict'](manifest, path, [path])
            self.assertTrue(ok, why)
            path.write_text(text.replace('phase=1 ', 'phase=5 '))
            self.assertFalse(scorer['verdict'](manifest, path, [path])[0])

    def test_uploaded_phase_records_are_verified_and_separated_from_approval_input(self):
        codec = runpy.run_path(str(ROOT / 'scripts/metal/screen-crc-export.py'))
        text, _, rows = fixture()
        rows = [dict(rows[0], checkpoint='end'), *rows]
        def encode(record):
            names = [record[k].encode() for k in ('suite', 'case', 'checkpoint')]
            methods = record['output_config']['methods']; samples = record['samples']
            header = bytearray(116); header[:8] = b'SCRCRC01'
            length = 116 + sum(map(len, names)) + 8 * len(methods) + 12 * len(samples)
            struct.pack_into('<III6H', header, 8, 1, length, 6, *map(len, names), len(methods), len(samples), 1)
            struct.pack_into('<4Q', header, 32, 7, 9, 7, 9)
            struct.pack_into('<13I', header, 64, 85000, 0, 1, 2, 1, 1280, 1024, 0, 112, 1280, 800, 0, 0)
            return (bytes(header) + b''.join(names) + b''.join(struct.pack('<2I', *m) for m in methods) +
                    b''.join(struct.pack('<3I', s['status'], int(s['crc'], 16), int(s['crc2'], 16)) for s in samples))
        with tempfile.TemporaryDirectory() as tmp:
            boot = Path(tmp); run = dict(id='fixture', commit=HEAD, screenProof=True)
            (boot / 'run.json').write_text(json.dumps(run)); (boot / 'commit').write_text(HEAD)
            anchors = []
            for row in rows:
                raw = encode(row)
                label = 'crc-gpu-' + hashlib.sha256(('draw/one\0' + row['checkpoint']).encode()).hexdigest()[:16]
                sha = hashlib.sha256(raw).hexdigest()
                (boot / ('diagnostic-' + label + '.bin')).write_bytes(raw)
                (boot / ('native-diagnostic-' + label + '.json')).write_text(json.dumps(dict(
                    id=run['id'], bytes=len(raw), sha256=sha, complete=True)))
                anchors.append(f"SCREEN_CRC_UPLOAD gpu draw/one {row['checkpoint']} label={label} bytes={len(raw)} sha256={sha} result=ok")
            (boot / 'serial.txt').write_text(text + '\n'.join(anchors) + '\n')
            normal, phases = codec['export'](boot)
            self.assertEqual([r['checkpoint'] for r in normal], ['end'])
            self.assertEqual([r['checkpoint'] for r in phases], list(codec['PHASE_CHECKPOINTS']))
            self.assertEqual(json.loads((boot / 'screen-crc.json').read_text())['checkpoints'], normal)
            self.assertEqual(json.loads((boot / 'screen-perturb.json').read_text())['checkpoints'], phases)
            API['check_perturbation']('gpu', text, fixture()[1], phases)
            artifact = boot / ('diagnostic-' + label + '.bin')
            artifact.write_bytes(artifact.read_bytes()[:-1])
            with self.assertRaisesRegex(ValueError, 'digest'): codec['export'](boot)

    def test_native_gpu_and_breengel_parsers_admit_phase_records(self):
        from gt710_resources_report_test import transcript, boot as gpu_boot
        from gt710_breengel_evidence_test import composition_capture
        from gt710_breengel_selection_test import boot as breengel_boot
        native = runpy.run_path(str(ROOT / 'scripts/metal/gt710-suite'))
        for suite, serial, boot in [('gpu', transcript(all_pass=True), gpu_boot()),
                                     ('breengel', composition_capture(), breengel_boot())]:
            boot = dict(boot, screenProof=True)
            serial = serial.replace(f'SUITE {suite} START cases=', f'SUITE {suite} START cases=', 1)
            start = next(line for line in serial.splitlines() if line.startswith(f'SUITE {suite} START '))
            serial = serial.replace(start, start + ' screen_proof=on', 1)
            case = next(line.split()[3] for line in serial.splitlines() if line.startswith(f'SUITE {suite} CASE '))
            phases = '\n'.join(line for line in fixture(suite)[0].splitlines() if ' SCREENPERTURB ' in line)
            phases = phases.replace('draw/one', case) + '\n'
            serial = serial.replace(f'SUITE {suite} DONE ', phases + f'SUITE {suite} DONE ')
            score = (lambda text: native['resources_suite'](text, boot)) if suite == 'gpu' else (
                lambda text: native['breengel_fronts_suite'](text, boot, 4))
            with self.subTest(suite=suite):
                score(serial)
                with self.assertRaises(ValueError): score(serial.replace('phase=1 ', 'phase=5 '))


if __name__ == '__main__':
    unittest.main()
