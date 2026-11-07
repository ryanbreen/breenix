"""Exercise table placement after migrating a populated legacy fixture directory."""
import os
from pathlib import Path
import subprocess
import tempfile
import tomllib
import unittest

ROOT = Path(__file__).resolve().parents[1]


class FixtureInstall(unittest.TestCase):
    def test_uitest_app_uses_main_layout(self):
        with tempfile.TemporaryDirectory() as scratch:
            base = Path(scratch)
            root, binaries = base / 'root', base / 'binaries'
            root.mkdir()
            binaries.mkdir()
            cargo = tomllib.loads((ROOT / 'userspace/programs/Cargo.toml').read_text())
            self.assertTrue(any(b['name'] == 'uitest-app_test' and b['path'] == 'src/uitest_app.rs'
                                for b in cargo['bin']))
            (binaries / 'uitest-app_test.elf').write_bytes(b'app ELF')
            layout = base / 'layout.tsv'
            layout.write_bytes(subprocess.check_output(['git', 'show', 'origin/main:libs/libbreenix/src/layout.tsv'], cwd=ROOT))
            self.assertEqual(layout.read_bytes(), (ROOT / 'libs/libbreenix/src/layout.tsv').read_bytes())
            result = subprocess.run(['sh', str(ROOT / 'scripts/install-layout.sh'), str(root),
                                     str(binaries), str(layout)],
                                    env=dict(os.environ, BREENIX_EXT2_OWNERS=str(base / 'owners')),
                                    capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual((root / 'usr/libexec/breenix/uitest-app_test').read_bytes(), b'app ELF')
            self.assertFalse((root / 'sbin/uitest-app_test').exists())
            self.assertFalse((root / 'usr/local').exists())

    def test_declared_fixture_stays_at_its_path_after_legacy_migration(self):
        with tempfile.TemporaryDirectory() as scratch:
            base = Path(scratch)
            root, binaries = base / 'root', base / 'binaries'
            legacy = root / 'usr/local/test/bin'
            legacy.mkdir(parents=True)
            (legacy / 'legacy_test').write_bytes(b'legacy')
            binaries.mkdir()
            cargo = tomllib.loads((ROOT / 'userspace/programs/Cargo.toml').read_text())
            fixture = next(binary['name'] for binary in cargo['bin']
                           if binary['path'] == 'src/suite_runner_fixture.rs')
            (binaries / (fixture + '.elf')).write_bytes(b'fixture ELF')
            # Ownership is recorded for the image packer on an unprivileged host.
            environment = dict(os.environ, BREENIX_EXT2_OWNERS=str(base / 'owners'))
            result = subprocess.run(['sh', str(ROOT / 'scripts/install-layout.sh'), str(root),
                                     str(binaries), str(ROOT / 'libs/libbreenix/src/layout.tsv')],
                                    env=environment, capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            installed = root / 'usr/libexec/breenix/ui-runner-fixture_test'
            self.assertTrue(installed.is_file(), 'table placement was retired by legacy migration')
            self.assertEqual(installed.read_bytes(), b'fixture ELF')
            self.assertEqual(installed.stat().st_mode & 0o7777, 0o755)
            self.assertFalse((root / 'sbin' / fixture).exists())
            self.assertFalse((root / 'usr/local/test').exists())
            self.assertEqual((root / 'usr/libexec/breenix/legacy_test').read_bytes(), b'legacy')


if __name__ == '__main__':
    unittest.main()
