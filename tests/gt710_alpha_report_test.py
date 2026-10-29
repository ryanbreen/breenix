"""PR 8.3 selection and retained-capture scorer refusal checks."""
import hashlib
import json
from pathlib import Path
import re
import runpy
import shutil
import tempfile
import unittest

from gt710_resources_report_test import API, ROOT, SHA, NAMES, REQUIRED, PROBE_FILES
from gt710_resources_report_test import boot, events, probes, stage, transcript

ALPHA = {'pipelines/' + name for name in (
    'over-premultiplied', 'over-straight', 'over-overlap',
    'coverage-blend', 'coverage-lcd', 'bitmap-text')}
SELECTED = [n for n in NAMES if n in REQUIRED or n in ALPHA or
            (n.startswith('profile/') and n not in (
                'profile/binary32-emulation', 'profile/large-coordinates',
                'profile/scissor-exact'))]


def alpha_transcript():
    text = transcript()
    for name in SELECTED:
        text = text.replace(f'CASE {name} FAIL ms=1 msg=not implemented',
                            f'CASE {name} PASS ms=1')
    return text.replace('passed=52 failed=118', 'passed=77 failed=93')


def retained_capture(directory):
    gpu = directory / 'resources-producer'
    gpu.mkdir()
    for path, suite in ((directory, 'console'), (gpu, 'gpu')):
        raw_boot = boot(suite)
        del raw_boot['bootedCommit']
        serial = transcript('console')
        if suite == 'gpu':
            stage(path)
            serial = (path / 'serial.txt').read_text() + alpha_transcript()
            # G12: real reprobe records are uploaded after the cases, with
            # individual serial anchors and the final-slot digest.
            rows = [dict(p, phase='caps-reprobe', epoch=17) for p in probes()]
            body = b''
            anchors = ''
            for row in rows:
                raw = json.dumps(row, separators=(',', ':')).encode()
                body += raw + b'\n'
                anchors += (f'GT710_CAPS_REPROBE id={row["capability"]} epoch=17 '
                            f'sha256={hashlib.sha256(raw).hexdigest()}\n')
            (path / 'gt710-final.rle').write_bytes(body)
            final = f'GT710_FINAL_BULK {len(body)} {hashlib.sha256(body).hexdigest()}\n'
            serial = serial.replace('SUITE gpu ELAPSED', anchors + final + 'SUITE gpu ELAPSED')
        for name, value in {
            'run.json': json.dumps(raw_boot), 'serial.txt': serial, 'commit': SHA,
            'dirty': '0', 'exit-status': '1' if suite == 'gpu' else '0', 'ended': '1',
            'events.jsonl': ''.join(json.dumps(e) + '\n' for e in events()),
        }.items():
            (path / name).write_text(value)
    files = [
        'docs/metal/boot-path.json', 'docs/boot-path.json', 'docs/efforts/metal.json',
        'docs/suites/gt710.json', 'docs/suites/gpu.json', 'docs/suites/console.json',
        'docs/gui/gt710-step-8-2.json', 'docs/gui/gt710-step-8-3.json',
        'scripts/metal/gt710-suite', 'scripts/suite-verdict.py',
        'tools/breenix-runs/Sources/BreenixRuns/Resources/boot-stages-x86_64.json',
    ] + ['kernel/src/gpu/gk208/caps/' + name for name in set(PROBE_FILES)]
    for name in files:
        target = directory / 'definition' / name
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(ROOT / name, target)
    return gpu


class AlphaReport(unittest.TestCase):
    def test_selection_and_registered_row(self):
        step = json.loads((ROOT / 'docs/gui/gt710-step-8-3.json').read_text())
        self.assertEqual((step['id'], step['suite']), ('gt710-step-8-3', 'gpu'))
        self.assertEqual(step['cases'], SELECTED)
        self.assertEqual(len(SELECTED), 77)
        # G15 keeps device-error production and its dependent cases in 8.5.
        later = json.loads((ROOT / 'docs/gui/gt710-step-8-5.json').read_text())
        self.assertIn('profile/scissor-exact', later['cases'])
        self.assertIn('textures/copy-region', later['cases'])
        self.assertFalse(set(step['cases']) & set(later['cases']))
        manifest = json.loads((ROOT / 'docs/suites/gt710.json').read_text())
        API['validate_manifest'](manifest)
        category = next(c for c in manifest['categories'] if c['id'] == 'alpha')
        self.assertTrue(category['hostRunner'])
        self.assertEqual([c['id'] for c in category['cases']], ['suite'])
        self.assertNotIn('alpha', API['capture_categories'](boot('console')))
        self.assertIn('docs/gui/gt710-step-8-3.json',
                      (ROOT / 'scripts/metal/metal-build').read_text())

    def test_alpha_row_judges_each_selected_case(self):
        producer = dict(gpu=dict(serial=alpha_transcript(), boot=boot()),
                        console=dict(serial=transcript('console'), boot=boot('console'), events=events()))
        result = API['cases']('alpha', '', producer)
        self.assertEqual([(c['id'], c['result']) for c in result], [('alpha/suite', 'PASS')])
        for name in SELECTED:
            changed = alpha_transcript().replace(f'CASE {name} PASS ms=1',
                                                f'CASE {name} FAIL ms=1 msg=broken')
            changed = changed.replace('passed=77 failed=93', 'passed=76 failed=94')
            producer['gpu']['serial'] = changed
            with self.subTest(case=name):
                self.assertEqual(API['cases']('alpha', '', producer)[0]['result'], 'FAIL')

    def test_retained_definition_scores_alpha_and_refuses_bad_evidence(self):
        metal = runpy.run_path(str(ROOT / 'scripts/metal/metal_score.py'))
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp)
            gpu = retained_capture(directory)
            self.assertEqual(API['record_resources_capture'](directory, SHA, 'alpha'), 0)
            report = json.loads((directory / 'gt710-alpha.json').read_text())
            self.assertEqual(sum(c['result'] == 'PASS' for c in report['gpuCases']), 77)
            def score():
                return metal['score'](directory)['milestones']['metal-gt710/alpha']
            self.assertEqual(score(), dict(reached=1, total=1, passed=True, reason='alpha: 1 cases passed'))
            original = (gpu / 'serial.txt').read_text()
            changes = {
                'selected failure': original.replace('CASE profile/wide-arithmetic PASS ms=1',
                    'CASE profile/wide-arithmetic FAIL ms=1 msg=broken').replace(
                        'passed=77 failed=93', 'passed=76 failed=94'),
                'missing DONE': re.sub(r'^SUITE gpu DONE .*\n', '', original, flags=re.M),
                'unmatched G12 anchor': original.replace('GT710_FINAL_BULK ',
                    'GT710_CAPS_REPROBE id=3 epoch=18 sha256=' + '0'*64 + '\nGT710_FINAL_BULK '),
            }
            for reason, text in changes.items():
                (gpu / 'serial.txt').write_text(text)
                with self.subTest(reason=reason):
                    self.assertFalse(score()['passed'])
            (gpu / 'serial.txt').write_text(original)


if __name__ == '__main__':
    unittest.main()
