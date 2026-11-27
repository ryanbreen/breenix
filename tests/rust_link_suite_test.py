"""K6 suite table and fresh Mac bld replay contract."""
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
FIXTURE = Path(os.environ.get('BREENIX_K6_FIXTURE', ROOT / 'build/rustc-port/k6/fixture'))

AARCH64_FIXTURE = Path(os.environ.get('BREENIX_K6_AARCH64_FIXTURE', Path.home() / 'Library/Caches/breenix-e25/k6/aarch64'))

class RustLinkSuite(unittest.TestCase):
    def test_suite_and_replay(self):
        manifest = json.loads((ROOT / 'docs/suites/rust-link.json').read_text())
        source = (ROOT / 'userspace/programs/src/suite_rust_link.rs').read_text()
        ids = re.findall(r'case\("([^"]+)"', source)
        self.assertEqual(ids, ['links-tool-target', 'links-hosted-unwind', 'rlib-members-skipped'])
        self.assertEqual(ids, [case['id'] for case in manifest['categories'][0]['cases']])
        self.assertEqual(manifest['categories'][0]['id'], 'rust-link')
        expect = json.loads((ROOT / 'toolchain/rust-host/k6/expect.json').read_text())
        record = json.loads((ROOT / 'toolchain/rust-host/k6/x86_64.json').read_text())
        bld = Path(os.environ.get('BREENIX_K6_BLD', ROOT / 'toolchain/target/release/bld')).resolve()
        with tempfile.TemporaryDirectory() as directory:
            for name in ('panic-abort', 'catch-unwind'):
                args = record['links'][name]['argv'].copy()
                output = Path(directory) / name
                args[args.index('-o') + 1] = str(output)
                subprocess.run([str(bld), *args], cwd=FIXTURE, env={}, check=True)
                self.assertEqual(hashlib.sha256(output.read_bytes()).hexdigest(), expect[name]['bld_sha256'])
                reference = FIXTURE / record['links'][name]['argv'][record['links'][name]['argv'].index('-o') + 1]
                self.assertEqual(hashlib.sha256(reference.read_bytes()).hexdigest(), expect[name]['reference_sha256'])

    def test_rlib_members_skipped(self):
        records = ROOT / 'toolchain/rust-host/k6'
        expect = json.loads((records / 'expect.json').read_text())
        record = json.loads((records / 'x86_64.json').read_text())
        bld = Path(os.environ.get('BREENIX_K6_BLD', ROOT / 'toolchain/target/release/bld')).resolve()
        metadata = records / expect['metadata']['archive']
        control = expect['control']
        with tempfile.TemporaryDirectory() as directory:
            args = record['links']['panic-abort']['argv'].copy()
            destination = args.index('-o') + 1
            positive = Path(directory) / 'positive'
            args[destination] = str(positive)
            subprocess.run([str(bld), *args, '--whole-archive', str(metadata), '--no-whole-archive'],
                           cwd=FIXTURE, env={}, check=True)
            self.assertEqual(hashlib.sha256(positive.read_bytes()).hexdigest(),
                             expect['panic-abort']['bld_sha256'])

            negative = Path(directory) / 'control'
            args[destination] = str(negative)
            args[args.index(control['original'])] = control['archive']
            result = subprocess.run([str(bld), *args], cwd=FIXTURE, env={}, capture_output=True)
            self.assertGreater(result.returncode, 0)
            self.assertEqual(result.stderr, control['diagnostic'].encode())
            self.assertFalse(negative.exists())

    def test_aarch64_suite_and_replay(self):
        manifest = json.loads((ROOT / 'docs/suites/rust-link.json').read_text())
        source = (ROOT / 'userspace/programs/src/suite_rust_link.rs').read_text()
        ids = re.findall(r'case\("([^"]+)"', source)
        self.assertEqual(ids, ['links-tool-target', 'links-hosted-unwind', 'rlib-members-skipped'])
        self.assertEqual(ids, [case['id'] for case in manifest['categories'][0]['cases']])
        self.assertEqual(manifest['categories'][0]['id'], 'rust-link')
        expect = json.loads((ROOT / 'toolchain/rust-host/k6/expect-aarch64.json').read_text())
        record = json.loads((ROOT / 'toolchain/rust-host/k6/aarch64.json').read_text())
        bld = Path(os.environ.get('BREENIX_K6_BLD', ROOT / 'toolchain/target/release/bld')).resolve()
        with tempfile.TemporaryDirectory() as directory:
            for name in ('panic-abort', 'catch-unwind'):
                args = record['links'][name]['argv'].copy()
                output = Path(directory) / name
                args[args.index('-o') + 1] = str(output)
                subprocess.run([str(bld), *args], cwd=AARCH64_FIXTURE, env={}, check=True)
                self.assertEqual(hashlib.sha256(output.read_bytes()).hexdigest(), expect[name]['bld_sha256'])
                reference = AARCH64_FIXTURE / record['links'][name]['argv'][record['links'][name]['argv'].index('-o') + 1]
                self.assertEqual(hashlib.sha256(reference.read_bytes()).hexdigest(), expect[name]['reference_sha256'])

    def test_aarch64_rlib_members_skipped(self):
        records = ROOT / 'toolchain/rust-host/k6'
        expect = json.loads((records / 'expect-aarch64.json').read_text())
        record = json.loads((records / 'aarch64.json').read_text())
        bld = Path(os.environ.get('BREENIX_K6_BLD', ROOT / 'toolchain/target/release/bld')).resolve()
        metadata = records / expect['metadata']['archive']
        control = expect['control']
        with tempfile.TemporaryDirectory() as directory:
            args = record['links']['panic-abort']['argv'].copy()
            destination = args.index('-o') + 1
            positive = Path(directory) / 'positive'
            args[destination] = str(positive)
            subprocess.run([str(bld), *args, '--whole-archive', str(metadata), '--no-whole-archive'],
                           cwd=AARCH64_FIXTURE, env={}, check=True)
            self.assertEqual(hashlib.sha256(positive.read_bytes()).hexdigest(),
                             expect['panic-abort']['bld_sha256'])

            negative = Path(directory) / 'control'
            args[destination] = str(negative)
            args[args.index(control['original'])] = control['archive']
            result = subprocess.run([str(bld), *args], cwd=AARCH64_FIXTURE, env={}, capture_output=True)
            self.assertGreater(result.returncode, 0)
            self.assertEqual(result.stderr, control['diagnostic'].encode())
            self.assertFalse(negative.exists())
