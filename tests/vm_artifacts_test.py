"""Reject stale or replaced VM build outputs before admitting boot evidence."""
import json
from pathlib import Path
import runpy
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
ARTIFACTS = runpy.run_path(str(ROOT / 'scripts/vm-artifacts.py'))


class VmArtifacts(unittest.TestCase):
    def test_imported_old_outputs_cannot_attest_the_new_head(self):
        with tempfile.TemporaryDirectory() as name:
            root = Path(name)
            repo, run = root / 'repo', root / 'run'
            repo.mkdir(); run.mkdir()
            def git(*args):
                return subprocess.check_output(['git', '-C', str(repo), *args], text=True).strip()
            git('init', '-q'); git('config', 'user.name', 'Fixture')
            git('config', 'user.email', 'fixture@example.invalid')
            source = repo / 'kernel.rs'
            source.write_text('A')
            git('add', '.'); git('commit', '-qm', 'Build A')
            a = git('rev-parse', 'HEAD')
            for path in ('kernel', 'system.img', 'home.img'):
                (run / path).write_bytes(b'artifact A ' + path.encode())
            ARTIFACTS['seal'](repo, run, a)
            receipt = ARTIFACTS['verify'](repo, run, run / 'kernel')
            self.assertTrue(receipt['evidenceEligible'])
            source.write_text('B'); git('commit', '-qam', 'Change kernel B')
            with self.assertRaisesRegex(ValueError, 'clean tools commit'):
                ARTIFACTS['verify'](repo, run, run / 'kernel')
            # Packaging A's arbitrary prebuilt artifacts at clean B is ineligible.
            ARTIFACTS['seal'](repo, run, '')
            self.assertFalse(ARTIFACTS['verify'](repo, run, run / 'kernel')['evidenceEligible'])
            b = git('rev-parse', 'HEAD')
            ARTIFACTS['seal'](repo, run, b)
            for path in ('kernel', 'system.img', 'home.img'):
                original = (run / path).read_bytes()
                (run / path).write_bytes(b'replaced')
                with self.assertRaisesRegex(ValueError, 'artifacts differ'):
                    ARTIFACTS['verify'](repo, run, run / 'kernel')
                (run / path).write_bytes(original)
            self.assertEqual(json.loads((run / 'build.json').read_text())['commit'], b)


if __name__ == '__main__':
    unittest.main()
