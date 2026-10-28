"""Production suite boots and explicit core diagnostics have separate evidence."""
import itertools
import json
from pathlib import Path
import runpy
from types import SimpleNamespace
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
RUN = runpy.run_path(str(ROOT / 'scripts/metal/metal-run'))
BOOT = runpy.run_path(str(ROOT / 'scripts/metal/metal-boot'))
SCORE = runpy.run_path(str(ROOT / 'scripts/metal/metal_score.py'))
CORE = runpy.run_path(str(ROOT / 'tests/gt710_core_report_test.py'))


class Profiles(unittest.TestCase):
    def test_suite_combinations_select_production(self):
        selections = ['all', None]
        names = ['console', 'gpu', 'gt710', 'smoke']
        selections += [','.join(c) for n in range(1, 5) for c in itertools.permutations(names, n)]
        for suites in selections:
            with self.subTest(suites=suites):
                self.assertEqual(RUN['kernel_features']('suite', suites, None), [])
                args = SimpleNamespace(mode='suite', suite=suites, inject=None)
                self.assertEqual(BOOT['kernel_profile'](args), ([], 'production'))
        sequence = RUN['sequence_tools']()
        resolved, _ = sequence['resolve']('all', 'x86-metal', True)
        self.assertIn('console', resolved)
        self.assertIn('gpu', resolved)
        self.assertEqual(RUN['kernel_features']('suite', ','.join(resolved), None), [])

    def test_only_named_core_diagnostic_selects_probe(self):
        args = SimpleNamespace(mode='default', suite=None, inject=None, gt710_category='core')
        self.assertEqual(RUN['kernel_features']('default', None, None, 'core'),
                         ['--features', 'gt710_core_probe'])
        self.assertEqual(BOOT['kernel_profile'](args), (['gt710_core_probe'], 'gpu-core'))
        for suites in ['console', 'gpu', 'console,gpu', 'gt710', 'all']:
            with self.subTest(suites=suites), self.assertRaisesRegex(ValueError, 'core.*default'):
                RUN['kernel_features']('suite', suites, None, 'core')
            args.mode, args.suite = 'suite', suites
            with self.subTest(driver=suites), self.assertRaisesRegex(ValueError, 'core.*default'):
                BOOT['kernel_profile'](args)

    def test_suite_scorers_require_production_profile(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp)
            files = {
                'definition/docs/metal/boot-path.json': json.dumps(dict(gateArch='x86_64', milestones=[])),
                'definition/tools/breenix-runs/Sources/BreenixRuns/Resources/boot-stages-x86_64.json': '{"stages":[]}',
                'serial.txt': '', 'events.jsonl': '',
            }
            for name, text in files.items():
                dest = path / name
                dest.parent.mkdir(parents=True, exist_ok=True)
                dest.write_text(text)
            for suites in [['console'], ['gpu'], ['console', 'gpu'], ['smoke', 'gpu']]:
                for profile in [None, 'gpu-core', 'testing', 'production']:
                    boot = dict(mode='suite', disk='root', suite=suites[0] if len(suites) == 1 else None,
                                suites=suites)
                    if profile is not None:
                        boot['build'] = dict(profile=profile)
                    (path / 'run.json').write_text(json.dumps(boot))
                    with self.subTest(suites=suites, profile=profile):
                        if profile == 'production':
                            self.assertTrue(SCORE['score'](path)['captureComplete'])
                        else:
                            with self.assertRaisesRegex(ValueError, 'production.*build.profile'):
                                SCORE['score'](path)
                        driver = object.__new__(BOOT['Driver'])
                        driver.args = SimpleNamespace(mode='suite', suite=','.join(suites))
                        driver.dir = path
                        with patch.dict(BOOT['Driver'].score_suite.__globals__,
                                        subprocess=SimpleNamespace(run=lambda *a, **k: self.fail('scorer spawned'))):
                            if profile != 'production':
                                self.assertEqual(driver.score_suite(), 1)

    def test_core_and_producer_capture_profiles_are_required(self):
        api = CORE['API']
        boot = CORE['gpu_boot']()
        caps = api['core_row'](''.join('GT710_CORE ' + json.dumps(r) + '\n' for r in CORE['records']()))
        for profile in [None, 'gpu-core', 'testing']:
            wrong = dict(boot)
            wrong['build'] = dict(profile=profile)
            with self.subTest(producer=profile), self.assertRaisesRegex(ValueError, 'production.*build.profile'):
                api['core_producer'](CORE['gpu_serial'](), caps, wrong)
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp)
            diagnostic = dict(boot, mode='default', suite=None, suites=None, gt710Category='core',
                              build=dict(profile='gpu-core', features=['gt710_core_probe']))
            for name, text in {'run.json':json.dumps(diagnostic), 'commit':CORE['SHA'], 'dirty':'0',
                               'ended':'1', 'exit-status':'0', 'events.jsonl':'{"event":"final-marker"}\n'}.items():
                (path / name).write_text(text)
            CORE['stage'](path)
            self.assertIn('GT710_CORE ', api['core_diagnostic_capture'](path, CORE['SHA'])['serial'])
            for profile in [None, 'production', 'testing']:
                diagnostic['build'] = dict(profile=profile)
                (path / 'run.json').write_text(json.dumps(diagnostic))
                with self.subTest(diagnostic=profile), self.assertRaisesRegex(ValueError, 'gpu-core.*build.profile'):
                    api['core_diagnostic_capture'](path, CORE['SHA'])


if __name__ == '__main__':
    unittest.main()
