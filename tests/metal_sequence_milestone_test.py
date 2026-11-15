"""Milestone selection for multi-suite metal boots, without hardware."""
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

import metal_tools_test as existing

boot, load_script = existing.boot, existing.load_script
from metal_tests_mode_test import run_args

ROOT = Path(__file__).resolve().parents[1]


class SequenceMilestoneTest(unittest.TestCase):
    def test_launch_requires_the_commit_milestone_suite_in_the_list(self):
        run = load_script('metal-run')
        path = (ROOT / 'docs/efforts/path.json').read_text()
        def git_show(env, *args, **kwargs):
            self.assertEqual(args[-1], 'a' * 40 + ':docs/efforts/path.json')
            return SimpleNamespace(returncode=0, stdout=path, stderr='')
        with tempfile.TemporaryDirectory() as home, \
             patch.object(run, 'host_env', return_value={'BUILD_REPO': '/repo'}), \
             patch.object(run, 'resolve', return_value='a' * 40), \
             patch.object(run, 'resolve_suites', side_effect=lambda env, sha, spec: spec), \
             patch.object(run, 'in_build', side_effect=git_show), \
             patch.object(run, 'in_metal', return_value=SimpleNamespace(returncode=0, stdout='', stderr='')) as metal, \
             patch.object(run.Path, 'home', return_value=Path(home)), \
             patch.object(run.subprocess, 'Popen'):
            args = run_args(mode='default', suite='console,filesystem,directories', milestone='filesystem/fs-interface')
            self.assertEqual(run.boot(args), 0)
            command = metal.call_args.args
            self.assertEqual(command[command.index('--milestone') + 1], 'filesystem/fs-interface')
            with self.assertRaises(SystemExit):
                run.boot(run_args(mode='default', suite='console,directories', milestone='filesystem/fs-interface'))

    def test_sequence_scorer_receives_suite_qualified_selector(self):
        fixture = existing.DriverTest()
        fixture.setUp()
        try:
            driver = fixture.driver
            driver.args.mode, driver.args.suite = 'suite', 'console,filesystem,directories'
            driver.args.milestone = 'filesystem/fs-interface'
            definition = driver.dir / 'definition'
            names = ['scripts/suite-verdict.py', 'scripts/suite-sequence.py', 'docs/boot-path.json',
                     'libs/libbreenix/src/suite_memory.rs', 'docs/efforts/path.json']
            names += ['docs/suites/' + suite + '.json' for suite in driver.args.suite.split(',')]
            for name in names:
                target = definition / name
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_text((ROOT / name).read_text() if name == 'docs/efforts/path.json' else '{}')
            (driver.dir / 'serial.txt').write_text('')
            with patch.object(boot.subprocess, 'run', return_value=SimpleNamespace(returncode=0, stdout='', stderr='')) as scorer:
                self.assertTrue(driver.score_suite())
            command = scorer.call_args.args[0]
            self.assertEqual(command[command.index('--milestone') + 1], 'filesystem=filesystem/fs-interface')
        finally:
            fixture.tearDown()


if __name__ == '__main__':
    unittest.main()
