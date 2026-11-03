#!/usr/bin/env python3
"""Build-only artifact handoff tests; no boot host or hardware access."""
import json
from pathlib import Path
import sys
import tempfile
import unittest
import os
import runpy
import subprocess
from types import SimpleNamespace
from unittest.mock import patch

from metal_tools_test import load_script, StateTest, boot, m

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'scripts/metal'))
import metal_prebuilt as p


class ManifestTest(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.commit, self.tools = 'a' * 40, 'b' * 40
        self.build = dict(commit=self.commit, profile='production', features=[], suite='console',
                          definition=['docs/boot-path.json'])
        for name in ('uefi.img', 'ext2.img', 'stub.efi', 'manager.efi', 'sha512-crypt',
                     'definition/docs/boot-path.json'):
            path = self.root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(name.encode())
        (self.root / 'build.json').write_text(json.dumps(self.build))
        p.create(self.root, self.tools, 'suite', dict(rustc='pinned', cargo='pinned'),
                 dict(commit='c' * 40), 'd' * 40)

    def verify(self, **changes):
        return p.verify(self.root, **(dict(commit=self.commit, tools_commit=self.tools,
                                          mode='suite', features=[], suite='console') | changes))

    def test_manifest_covers_every_file_and_identity(self):
        manifest = self.verify()
        self.assertEqual(manifest['builder'], 'mac')
        self.assertEqual(manifest['profile'], 'production')
        self.assertEqual(manifest['toolsCommit'], self.tools)
        self.assertEqual(manifest['rustPin']['commit'], 'c' * 40)
        self.assertEqual(set(manifest['artifacts']), {str(f.relative_to(self.root))
                         for f in self.root.rglob('*') if f.is_file() and f.name != 'manifest.json'})
        self.assertEqual(manifest['artifacts']['stub.efi']['size'], len(b'stub.efi'))

    def test_changed_artifact_is_refused(self):
        (self.root / 'stub.efi').write_bytes(b'changed!')
        with self.assertRaisesRegex(ValueError, 'stub.efi'):
            self.verify()

    def test_wrong_commit_and_tools_commit_are_refused(self):
        for field in ('commit', 'tools_commit'):
            with self.subTest(field=field), self.assertRaisesRegex(ValueError, 'commit|toolsCommit'):
                self.verify(**{field: 'e' * 40})

    def test_missing_artifact_is_refused_even_if_inventory_was_edited(self):
        (self.root / 'ext2.img').unlink()
        with self.assertRaisesRegex(ValueError, 'ext2.img'):
            self.verify()
        manifest = json.loads((self.root / 'manifest.json').read_text())
        del manifest['artifacts']['ext2.img']
        (self.root / 'manifest.json').write_text(json.dumps(manifest))
        with self.assertRaisesRegex(ValueError, 'ext2.img'):
            self.verify()

    def test_wrong_mode_features_suite_and_extra_files_are_refused(self):
        for change in (dict(mode='tests'), dict(features=['testing']), dict(suite='time')):
            with self.subTest(change=change), self.assertRaises(ValueError):
                self.verify(**change)
        (self.root / 'unlisted').touch()
        with self.assertRaisesRegex(ValueError, 'inventory'):
            self.verify()

    def test_ready_marker_is_only_allowed_in_service_staging(self):
        (self.root / 'ready').touch()
        with self.assertRaisesRegex(ValueError, 'inventory'):
            self.verify()
        self.verify(staged=True)

    def test_host_refusal_is_recorded_before_any_staging(self):
        run = load_script('metal-run')
        (self.root / 'COMMIT').write_text(self.tools)
        args = SimpleNamespace(mode='suite', suite='console', inject=None)
        with patch.object(run, 'HERE', self.root), \
             patch.object(run, 'prebuilt_path', return_value=self.root), \
             patch.object(run.Path, 'home', return_value=self.root), \
             patch.object(run, 'in_metal', return_value=SimpleNamespace(stdout=self.tools)) as remote:
            (self.root / 'stub.efi').write_bytes(b'changed')
            with self.assertRaisesRegex(SystemExit, 'stub.efi'):
                run.verify_prebuilt({}, '20261103T000000Z-metal', self.commit, args)
            self.assertEqual(remote.call_count, 1)
            self.assertEqual(remote.call_args.args[1], 'cat')
        refusal = json.loads((self.root / '.local/state/breenix-metal/prebuilt-refusals.jsonl').read_text())
        self.assertIn('stub.efi', refusal['reason'])


class HostRecordTest(StateTest):
    def test_explicit_host_build_keeps_existing_record_schema(self):
        args = SimpleNamespace(id='20261103T000000Z-metal', mode='default', payload='image', target='main',
                               task='', commit='a' * 40, inject=None, suite=None, nmi=False)
        with patch.object(m, 'load_site', return_value={}), patch.object(m, 'now', return_value=100), \
             patch.object(boot, 'tools_commit', return_value='b' * 40):
            driver = boot.Driver(args)
            driver.create_record()
        expected = dict(id=args.id, platform='metal', arch='x86_64', mode='default', target={'main': {}},
                        commit=args.commit, catchup=False, taskName=args.id, watch=False, display=False,
                        startedAt=100, payload='image', kind='boot', inject=None, suite=None, suites=None,
                        milestone=None, gt710Category=None, sourceBoot=None, disk=None, toolsCommit='b' * 40,
                        entry=None, slotAction=None, adopted=False, resting=False, upgradeParent=None,
                        run=None, input=None, session=False, idleMinutes=None, evidenceEligible=True)
        self.assertEqual((driver.dir / 'run.json').read_bytes(),
                         (json.dumps(expected, indent=1, sort_keys=True) + '\n').encode())
        self.assertFalse((driver.dir / 'metal-run.json').exists())
        self.assertEqual(p.record_builder(expected), expected)


class PortableRootTest(unittest.TestCase):
    def test_unprivileged_root_preserves_bytes_modes_links_and_guest_ownership(self):
        import metal_root
        mkfs, debugfs = metal_root.require_e2fsprogs('mke2fs', 'debugfs')
        owners = runpy.run_path(str(Path(__file__).resolve().parents[1] / 'scripts/ext2-owners.py'))
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary) / 'root'
            home = root / 'etc/breenix/test-ssh/home/.ssh'
            home.mkdir(parents=True)
            (home / 'authorized_keys').write_bytes(b'fixture-key\n')
            (home / 'authorized_keys').chmod(0o600)
            (root / 'plain').write_bytes(b'payload\x00bytes')
            os.link(root / 'plain', root / 'linked')
            image = Path(temporary) / 'ext2.img'
            with image.open('wb') as output:
                output.truncate(4 << 20)
            subprocess.run([mkfs, '-q', '-t', 'ext2', '-F', '-d', str(root), str(image)], check=True)
            with patch.dict(os.environ, PATH=str(Path(debugfs).parent) + os.pathsep + os.environ['PATH']):
                owners['set_owners'](image)
            def stat(path):
                return subprocess.check_output([debugfs, '-R', 'stat ' + path, str(image)], text=True,
                                                stderr=subprocess.DEVNULL)
            self.assertRegex(stat('/plain'), r'User:\s+0\s+Group:\s+0')
            self.assertIn('Links: 2', stat('/linked'))
            self.assertRegex(stat('/etc/breenix/test-ssh/home/.ssh/authorized_keys'),
                             r'Mode:\s+0600')
            self.assertEqual(subprocess.check_output([debugfs, '-R', 'cat /plain', str(image)],
                                                     stderr=subprocess.DEVNULL), b'payload\x00bytes')


if __name__ == '__main__':
    unittest.main()
