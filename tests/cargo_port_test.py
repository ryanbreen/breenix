import json
import os
from pathlib import Path
import subprocess
import unittest

ROOT = Path(__file__).resolve().parents[1]
CACHE = Path.home() / 'Library/Caches/breenix-e25/k7'
PIN = '84709f085062cbf3c51fa507527c1b2334015178'


class CargoPort(unittest.TestCase):
    def test_x1_licenses_recorded(self):
        cargo = CACHE / 'cargo'
        commit = subprocess.check_output(
            ['git', '-C', str(cargo), 'rev-parse', 'HEAD'], text=True).strip()
        self.assertEqual(commit, PIN)
        env = dict(os.environ, CARGO_HOME=str(CACHE / 'cargo-home'))
        metadata = json.loads(subprocess.check_output(
            ['cargo', 'metadata', '--locked', '--offline', '--format-version', '1',
             '--manifest-path', str(cargo / 'Cargo.toml'), '--filter-platform',
             'x86_64-breenix-hosted', '--features', 'all-static'], env=env, text=True))
        record = json.loads((ROOT / 'toolchain/rust-host/x1-licenses.json').read_text())
        self.assertEqual(record['cargo_commit'], PIN)
        self.assertEqual(record['features'], ['all-static'])
        packages = {(entry['name'], entry['version']): entry for entry in record['packages']}
        self.assertEqual(len(packages), len(record['packages']))
        self.assertEqual(set(packages), {(p['name'], p['version']) for p in metadata['packages']})
        for package in metadata['packages']:
            entry = packages[package['name'], package['version']]
            self.assertTrue(entry['spdx'], package['name'])
            self.assertTrue(entry['source'], package['name'])
            self.assertEqual(entry['metadata_license'], package['license'])
        libraries = {entry['name']: entry for entry in record['vendored_c_libraries']}
        self.assertEqual(len(libraries), len(record['vendored_c_libraries']))
        self.assertEqual(set(libraries), {'zlib', 'curl', 'nghttp2', 'OpenSSL',
                                         'libgit2', 'libssh2', 'SQLite'})
        for library in libraries.values():
            for key in ('version', 'spdx', 'source', 'vendored_by'):
                self.assertTrue(library[key], (library['name'], key))


if __name__ == '__main__':
    unittest.main()
