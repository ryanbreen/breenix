"""Exercise table placement after migrating a populated legacy fixture directory."""
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class FixtureInstall(unittest.TestCase):
    def test_declared_fixture_stays_at_its_path_after_legacy_migration(self):
        with tempfile.TemporaryDirectory() as scratch:
            base = Path(scratch)
            root, binaries = base / 'root', base / 'binaries'
            legacy = root / 'usr/local/test/bin'
            legacy.mkdir(parents=True)
            (legacy / 'legacy_test').write_bytes(b'legacy')
            binaries.mkdir()
            fixture = 'suite-runner-fixture_test'
            (binaries / (fixture + '.elf')).write_bytes(b'fixture ELF')
            # Ownership is recorded for the image packer on an unprivileged host.
            environment = dict(os.environ, BREENIX_EXT2_OWNERS=str(base / 'owners'))
            result = subprocess.run(['sh', str(ROOT / 'scripts/install-layout.sh'), str(root),
                                     str(binaries), str(ROOT / 'libs/libbreenix/src/layout.tsv')],
                                    env=environment, capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            installed = root / 'usr/local/test/bin' / fixture
            self.assertTrue(installed.is_file(), 'table placement was retired by legacy migration')
            self.assertEqual(installed.read_bytes(), b'fixture ELF')
            self.assertEqual(installed.stat().st_mode & 0o7777, 0o755)
            self.assertFalse((root / 'usr/libexec/breenix' / fixture).exists())
            self.assertEqual((root / 'usr/libexec/breenix/legacy_test').read_bytes(), b'legacy')


if __name__ == '__main__':
    unittest.main()
