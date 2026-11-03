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
                          definition=['docs/boot-path.json', 'docs/suites/console.json'])
        for name in ('uefi.img', 'ext2.img', 'stub.efi', 'manager.efi', 'sha512-crypt',
                     'definition/docs/boot-path.json'):
            path = self.root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(name.encode())
        suite = self.root / 'definition/docs/suites/console.json'
        suite.parent.mkdir(parents=True)
        suite.write_text(json.dumps(dict(id='console')))
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

    def test_retry_accepts_only_ordered_built_suites(self):
        self.build['suite'] = 'console,gpu,time'
        (self.root / 'build.json').write_text(json.dumps(self.build))
        p.create(self.root, self.tools, 'suite', dict(rustc='pinned'), dict(commit='c' * 40), 'd' * 40)
        for suite in ('console,gpu,time', 'gpu,time', 'console,time', 'time'):
            with self.subTest(suite=suite):
                self.verify(suite=suite)
        for suite in ('gpu,console', 'gpu,gpu', 'users', None):
            with self.subTest(suite=suite), self.assertRaisesRegex(ValueError, 'suite mismatch'):
                self.verify(suite=suite)

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

    def test_live_admission_refusal_precedes_launch_and_delivery(self):
        run = load_script('metal-run')
        args = SimpleNamespace(mode='suite', suite='console', inject=None, catchup=True)
        with tempfile.TemporaryDirectory() as temporary:
            tools = Path(temporary)
            (tools / 'COMMIT').write_text(self.tools)
            with patch.object(run, 'HERE', tools), \
                 patch.object(run, 'prebuilt_path', return_value=self.root), \
                 patch.object(run, 'host_env', return_value={'BUILD_REPO': '/repo'}), \
                 patch.object(run.Path, 'home', return_value=tools), \
                 patch.object(run, 'in_metal', return_value=SimpleNamespace(stdout=self.tools)) as remote, \
                 patch.object(run, 'admit_prebuilt', side_effect=ValueError('deleted metal lane')) as admit:
                with self.assertRaisesRegex(SystemExit, 'deleted metal lane'):
                    run.verify_prebuilt({'BUILD_REPO': '/repo'}, '20261103T000000Z-metal', self.commit, args)
                admit.assert_called_once_with({'BUILD_REPO': '/repo'}, self.commit, True)
                self.assertEqual(remote.call_count, 1)
                self.assertEqual(run.deliver_prebuilt('20261103T000001Z-metal', self.commit,
                                 '20261103T000000Z-metal', 'suite', suite='console', catchup=True), 1)
                self.assertEqual(remote.call_args.args[3], 'inputs-failed')

    def test_mac_bootstrap_executes_the_committed_entry_point(self):
        local = load_script('metal-build-local')
        with tempfile.TemporaryDirectory() as temporary:
            repo = Path(temporary)
            entry = repo / 'scripts/metal/metal-build-local'
            entry.parent.mkdir(parents=True)
            entry.write_text('print("committed tools entry point")\n')
            subprocess.run(['git', 'init', '-q', str(repo)], check=True)
            subprocess.run(['git', '-C', str(repo), 'add', '.'], check=True)
            subprocess.run(['git', '-C', str(repo), '-c', 'user.name=Test', '-c', 'user.email=test@example.com',
                            'commit', '-qm', 'tools fixture'], check=True)
            entry.write_text('raise RuntimeError("dirty tools executed")\n')
            with patch.object(local, '__file__', str(entry)), patch.object(sys, 'argv', [str(entry)]):
                self.assertEqual(local.bootstrap(), 0)

    def test_service_verifies_before_root_staging_and_records_mac_builder(self):
        for changed in (False, True):
            with self.subTest(changed=changed), tempfile.TemporaryDirectory() as temporary:
                state = Path(temporary)
                args = SimpleNamespace(id='20261103T000000Z-metal', mode='suite', payload='image', target='main',
                                       task='', commit=self.commit, inject=None, suite='console', nmi=False,
                                       ramdisk=True, disk=None)
                with patch.object(m, 'STATE', state), patch.object(m, 'load_site', return_value={}), \
                     patch.object(boot, 'tools_commit', return_value=self.tools):
                    driver = boot.Driver(args)
                    driver.create_record()
                    inputs = driver.stage / 'inputs'
                    import shutil
                    shutil.copytree(self.root, inputs)
                    (inputs / 'ready').touch()
                    if changed:
                        (inputs / 'stub.efi').write_bytes(b'changed')
                    with patch.object(boot.metal_root, 'stage_root', return_value={}) as stage, \
                         patch.object(boot.metal_persist, 'layout', return_value=None), \
                         patch.object(boot.metal_image, 'extract_esp', return_value={}), \
                         patch.object(driver, 'assemble') as assemble:
                        if changed:
                            with self.assertRaisesRegex(boot.Finish, 'stub.efi'):
                                driver.prepare_payload()
                            stage.assert_not_called()
                            assemble.assert_not_called()
                        else:
                            driver.prepare_payload()
                            stage.assert_called_once()
                            assemble.assert_called_once()
                            run = m.read_json(driver.dir / 'run.json')
                            self.assertEqual(run['builder'], 'mac')
                            self.assertEqual(run['build'], dict(commit=self.commit, profile='production', features=[]))
                            self.assertEqual((driver.dir / 'run.json').read_bytes(),
                                             (driver.dir / 'metal-run.json').read_bytes())


class HostRecordTest(StateTest):
    def test_prebuilt_rechecks_deleted_origin_lane(self):
        run = load_script('metal-run')
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            repo, origin = root / 'repo', root / 'origin'
            subprocess.run(['git', 'init', '-q', '-b', 'main', str(repo)], check=True)
            subprocess.run(['git', 'init', '-q', '--bare', str(origin)], check=True)
            def git(*args):
                return subprocess.check_output(['git', '-C', str(repo), *args], text=True).strip()
            git('config', 'user.name', 'Test')
            git('config', 'user.email', 'test@example.com')
            (repo / 'scripts').mkdir()
            (repo / 'scripts/effort-target-guard.py').write_bytes(
                (Path(__file__).resolve().parents[1] / 'scripts/effort-target-guard.py').read_bytes())
            (repo / 'docs/efforts').mkdir(parents=True)
            for name in ('path.json', 'metal.json'):
                (repo / 'docs/efforts' / name).write_text('{"efforts": []}')
            git('add', '.')
            git('commit', '-qm', 'policy')
            git('remote', 'add', 'origin', str(origin))
            git('push', '-q', 'origin', 'main')
            git('checkout', '-qb', 'metal-fixture/test')
            (repo / 'payload').touch()
            git('add', '.')
            git('commit', '-qm', 'payload')
            sha = git('rev-parse', 'HEAD')
            git('push', '-q', 'origin', 'metal-fixture/test')
            def local(env, *command, **kwargs):
                return subprocess.run(command, **kwargs)
            with patch.object(run, 'in_build', side_effect=local), \
                 patch.dict(os.environ, BREENIX_TARGET_POLICY_CHECKED='x86-metal:' + sha):
                run.admit_prebuilt({'BUILD_REPO': str(repo)}, sha)
                git('push', '-q', 'origin', '--delete', 'metal-fixture/test')
                with self.assertRaisesRegex(ValueError, 'not first-parent reachable'):
                    run.admit_prebuilt({'BUILD_REPO': str(repo)}, sha)

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
    def test_root_recipe_comes_from_payload_even_with_tools_override(self):
        recipe = runpy.run_path(str(Path(__file__).resolve().parents[1] / 'scripts/gate-artifacts.py'))
        with tempfile.TemporaryDirectory() as temporary:
            repo = Path(temporary)
            (repo / 'testdata').mkdir()
            (repo / 'target').mkdir()
            (repo / 'testdata/ext2.img').write_bytes(b'payload root')
            calls = []
            with patch.dict(os.environ, CARGO_HOME=str(repo), BREENIX_EXT2_SCRIPT='/tools/scripts/create_ext2_disk.sh'), \
                 patch.dict(recipe['build'].__globals__, checked=lambda command, *a, **kw: calls.append(command)):
                recipe['build'](repo, repo, 'payload')
            self.assertEqual(calls[-1], [str(repo / 'scripts/create_ext2_disk.sh')])
            self.assertEqual((repo / 'target/ext2.img').read_bytes(), b'payload root')

    def test_unprivileged_root_preserves_bytes_modes_links_and_guest_ownership(self):
        import metal_root
        mkfs, debugfs = metal_root.require_e2fsprogs('mke2fs', 'debugfs')
        owners = runpy.run_path(str(Path(__file__).resolve().parents[1] / 'scripts/ext2-owners.py'))
        repo = Path(__file__).resolve().parents[1]
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary) / 'root'
            home = root / 'etc/breenix/test-ssh/home/.ssh'
            home.mkdir(parents=True)
            (home / 'authorized_keys').write_bytes(b'fixture-key\n')
            (home / 'authorized_keys').chmod(0o600)
            (root / 'plain').write_bytes(b'payload\x00bytes')
            os.link(root / 'plain', root / 'linked')
            record = Path(temporary) / 'owners'
            record.write_text(''.join(f'2000:2000\t{path}\n' for path in (home.parent, home, home / 'authorized_keys')))
            binaries = Path(temporary) / 'binaries'
            binaries.mkdir()
            (binaries / 'sudo.elf').write_bytes(b'fixture executable')
            (binaries / 'rust-std-child_test.elf').write_bytes(b'fixture child')
            subprocess.run(['sh', str(repo / 'scripts/install-layout.sh'), str(root), str(binaries),
                            str(repo / 'libs/libbreenix/src/layout.tsv')], check=True,
                           env=dict(os.environ, BREENIX_EXT2_OWNERS=str(record)))
            image = Path(temporary) / 'ext2.img'
            with image.open('wb') as output:
                output.truncate(4 << 20)
            subprocess.run([mkfs, '-q', '-t', 'ext2', '-F', '-d', str(root), str(image)], check=True)
            with patch.dict(os.environ, PATH=str(Path(debugfs).parent) + os.pathsep + os.environ['PATH']):
                owners['set_owners'](image, record, root)
            def stat(path):
                return subprocess.check_output([debugfs, '-R', 'stat ' + path, str(image)], text=True,
                                                stderr=subprocess.DEVNULL)
            self.assertRegex(stat('/plain'), r'User:\s+0\s+Group:\s+0')
            self.assertIn('Links: 2', stat('/linked'))
            self.assertRegex(stat('/var/log/auth.log'), r'User:\s+0\s+Group:\s+4')
            self.assertRegex(stat('/etc/breenix/test-users/ua'), r'User:\s+2001\s+Group:\s+2001')
            self.assertRegex(stat('/usr/bin/sudo'), r'Mode:\s+04755')
            self.assertIn('Links: 2', stat('/usr/libexec/breenix/rust-std-child_test'))
            self.assertEqual(subprocess.check_output([debugfs, '-R',
                b'cat /usr/libexec/breenix/rust-std-child_test\xff', str(image)],
                stderr=subprocess.DEVNULL), b'fixture child')
            self.assertRegex(stat('/etc/breenix/test-ssh/home/.ssh/authorized_keys'),
                             r'Mode:\s+0600')
            self.assertEqual(subprocess.check_output([debugfs, '-R', 'cat /plain', str(image)],
                                                     stderr=subprocess.DEVNULL), b'payload\x00bytes')


if __name__ == '__main__':
    unittest.main()
