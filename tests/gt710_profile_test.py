"""Production suite boots and explicit core diagnostics have separate evidence."""
import itertools
import json
from pathlib import Path
import runpy
import subprocess
from types import SimpleNamespace
import sys
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
RUN = runpy.run_path(str(ROOT / 'scripts/metal/metal-run'))
BOOT = runpy.run_path(str(ROOT / 'scripts/metal/metal-boot'))
SCORE = runpy.run_path(str(ROOT / 'scripts/metal/metal_score.py'))
CORE = runpy.run_path(str(ROOT / 'tests/gt710_core_report_test.py'))


class Profiles(unittest.TestCase):
    def test_named_core_cli_uses_the_automated_upload_route(self):
        selected = []
        launch = RUN['boot']
        def stop_before_remote():
            raise RuntimeError('stop before remote')
        def capture(args):
            selected.append(args)
            return launch(args)
        with patch.dict(launch.__globals__, host_env=stop_before_remote), \
             patch.dict(RUN['main'].__globals__, boot=capture), \
             patch.object(sys, 'argv', ['metal-run', 'boot', '--gt710-category', 'core']):
            with self.assertRaisesRegex(RuntimeError, 'stop before remote'):
                RUN['main']()
        self.assertEqual(selected[0].mode, 'probe')
        self.assertEqual(selected[0].gt710_category, 'core')

    def test_capture_records_verified_build_profile(self):
        for profile, features, category in [('production', [], None),
                                            ('gpu-core', ['gt710_core_probe'], 'core')]:
            with self.subTest(profile=profile), tempfile.TemporaryDirectory() as tmp:
                path = Path(tmp)
                inputs = path / 'inputs'
                inputs.mkdir()
                (inputs / 'ready').touch()
                build = dict(commit=CORE['SHA'], profile=profile, features=features)
                (inputs / 'build.json').write_text(json.dumps(build))
                (path / 'run.json').write_text('{"id":"fixture"}')
                driver = object.__new__(BOOT['Driver'])
                driver.dir = driver.stage = path
                driver.args = SimpleNamespace(payload='image', mode='default', inject=None,
                                              commit=CORE['SHA'], gt710_category=category)
                driver.delivery = {}
                driver.phase = lambda *a: None
                def delivered(*args, **kwargs):
                    raise RuntimeError('stop after provenance')
                driver.event = delivered
                with self.assertRaisesRegex(RuntimeError, 'stop after provenance'):
                    driver.prepare_payload()
                self.assertEqual(json.loads((path / 'run.json').read_text())['build'], build)

    def test_explicit_core_default_boot_reaches_probe(self):
        source = (ROOT / 'kernel/src/gpu/gk208/native.rs').read_text()
        start = source.index('pub fn suite_requested()')
        function = source[start:source.index('\n}', start) + 2]
        harness = '''
mod boot {
    pub mod intent { pub fn get() -> Option<&'static [u8]> { None } }
    pub mod target {
        pub struct Suite { pub id: String }
        pub enum Target { Suite(Suite) }
        pub fn parse(_: &str) -> Result<Target, ()> {
            Ok(Target::Suite(Suite { id: "console".into() }))
        }
    }
}
fn main() { assert!(suite_requested(), "explicit core default boot must run the producer"); }
'''
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp)
            (path / 'probe.rs').write_text(function + harness)
            subprocess.run(['taskpolicy', '-b', 'rustc', '--edition=2021', '-Dwarnings',
                            '--cfg', 'feature="gt710_core_probe"', str(path / 'probe.rs'),
                            '-o', str(path / 'probe')], check=True)
            subprocess.run([str(path / 'probe')], check=True)

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
        args.mode = 'probe'
        self.assertEqual(RUN['kernel_features']('probe', None, None, 'core'),
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
                        driver.event = lambda *a, **k: None
                        with patch.dict(BOOT['Driver'].score_suite.__globals__,
                                        subprocess=SimpleNamespace(run=lambda *a, **k: self.fail('scorer spawned'))):
                            if profile != 'production':
                                self.assertIs(driver.score_suite(), False)

    def test_core_and_producer_capture_profiles_are_required(self):
        api = CORE['API']
        boot = CORE['gpu_boot']()
        with tempfile.TemporaryDirectory() as tmp:
            caps = api['core_row'](CORE['stage'](Path(tmp)))
        for profile in [None, 'gpu-core', 'testing']:
            wrong = dict(boot)
            wrong['build'] = dict(profile=profile)
            with self.subTest(producer=profile), self.assertRaisesRegex(ValueError, 'production.*build.profile'):
                api['core_producer'](CORE['gpu_serial'](), caps, wrong)
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp)
            diagnostic = dict(boot, mode='probe', suite=None, suites=None, gt710Category='core',
                              build=dict(profile='gpu-core', features=['gt710_core_probe']))
            for name, text in {'run.json':json.dumps(diagnostic), 'commit':CORE['SHA'], 'dirty':'0',
                               'ended':'1', 'exit-status':'0', 'events.jsonl':'{"event":"final-marker"}\n'}.items():
                (path / name).write_text(text)
            CORE['stage'](path)
            self.assertIn('GT710_CORE ', api['core_diagnostic_capture'](path, CORE['SHA'])['serial'])
            original = (path / 'serial.txt').read_text()
            (path / 'serial.txt').write_text(original + 'KERNEL PANIC: diagnostic failed\n')
            with self.assertRaisesRegex(ValueError, 'fatal'):
                api['core_diagnostic_capture'](path, CORE['SHA'])
            (path / 'serial.txt').write_text(original)
            for profile in [None, 'production', 'testing']:
                diagnostic['build'] = dict(profile=profile)
                (path / 'run.json').write_text(json.dumps(diagnostic))
                with self.subTest(diagnostic=profile), self.assertRaisesRegex(ValueError, 'gpu-core.*build.profile'):
                    api['core_diagnostic_capture'](path, CORE['SHA'])


if __name__ == '__main__':
    unittest.main()
