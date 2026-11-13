import os
from pathlib import Path
import subprocess
import tempfile
import tomllib
import unittest

ROOT = Path(__file__).resolve().parents[1]

class CollabLanInstall(unittest.TestCase):
    def test_fixtures_are_installed(self):
        names = ('collab-lan-host', 'collab-lan-join')
        cargo = tomllib.loads((ROOT / 'userspace/programs/Cargo.toml').read_text())
        for name in names:
            self.assertTrue(any(b['name'] == name for b in cargo['bin']))
            self.assertIn('"' + name + '"', (ROOT / 'userspace/programs/build.sh').read_text())
        with tempfile.TemporaryDirectory() as tmp:
            base = Path(tmp)
            root, bins = base / 'root', base / 'bins'
            root.mkdir(); bins.mkdir()
            for name in names:
                (bins / (name + '.elf')).write_bytes(name.encode())
            subprocess.run(['sh', str(ROOT / 'scripts/install-layout.sh'), str(root), str(bins),
                            str(ROOT / 'libs/libbreenix/src/layout.tsv')], check=True,
                           env=dict(os.environ, BREENIX_EXT2_OWNERS=str(base / 'owners')),
                           capture_output=True)
            for name in names:
                self.assertEqual((root / 'usr/libexec/breenix' / name).read_bytes(), name.encode())

if __name__ == '__main__':
    unittest.main()
