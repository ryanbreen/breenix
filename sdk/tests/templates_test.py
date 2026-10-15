#!/usr/bin/env python3
"""Host tests for template tools; real target builds run in the SDK suite."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]


class TemplatesTest(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='breenix-templates-')
        self.addCleanup(self.temp.cleanup)
        self.work = Path(self.temp.name).resolve()
        self.sdk = self.work / 'SDK with spaces'
        self.sdk.mkdir()
        (self.sdk / 'VERSION').write_text('breenix-sdk 0.1.0 ' + 'a' * 40 + '\n')
        env = dict(os.environ, SDK_ROOT=str(ROOT), SDK_STAGE=str(self.sdk))
        subprocess.run(['/bin/sh', str(ROOT / 'sdk/build/stages/60-templates.sh')], env=env, check=True)

    def tool(self, name, *args, cwd=None):
        return subprocess.run([str(self.sdk / 'bin' / ('breenix-' + name)), *map(str, args)],
                              cwd=cwd or self.work, capture_output=True, text=True)

    def create(self, kind='c', name='example'):
        result = self.tool('new', kind, name)
        self.assertEqual(result.returncode, 0, result.stderr)
        return self.work / name

    def driver(self, arch, tool, body):
        path = self.sdk / 'bin' / (arch + '-breenix-' + tool)
        path.write_text('#!/usr/bin/env python3\n' + body)
        path.chmod(0o755)

    def test_all_templates_are_standalone_and_docs_identical(self):
        result = self.tool('new', '--list')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.split(), ['c', 'rust', 'rust-no-std'])
        for kind in result.stdout.split():
            directory = self.create(kind, 'app-' + kind)
            self.assertIn('name = "app-' + kind + '"', (directory / 'breenix.toml').read_text())
            for path in directory.rglob('*'):
                if path.is_file():
                    text = path.read_text()
                    self.assertNotIn('{{name}}', text)
                    self.assertNotIn(str(ROOT), text)
                    self.assertNotIn(str(self.sdk), text)
        self.assertEqual((ROOT / 'docs/sdk/quick-start.md').read_bytes(),
                         (self.sdk / 'share/doc/breenix-sdk/quick-start.md').read_bytes())

    def test_occupied_hidden_file_and_symlink_are_untouched(self):
        target = self.work / 'occupied'
        target.mkdir()
        (target / '.keep').write_bytes(b'untouched\x00')
        before = (target / '.keep').read_bytes()
        result = self.tool('new', 'c', target)
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(list(target.iterdir()), [target / '.keep'])
        self.assertEqual((target / '.keep').read_bytes(), before)
        link = self.work / 'linked'
        link.symlink_to(target, target_is_directory=True)
        self.assertNotEqual(self.tool('new', 'c', link).returncode, 0)
        target_file = self.work / 'afile'
        target_file.write_text('keep')
        self.assertNotEqual(self.tool('new', 'c', target_file).returncode, 0)
        self.assertEqual(target_file.read_text(), 'keep')

    def test_empty_directory_allowed_and_invalid_name_refused(self):
        target = self.work / 'empty'
        target.mkdir()
        self.assertEqual(self.tool('new', 'c', target).returncode, 0)
        invalid = self.work / 'bad"name'
        self.assertNotEqual(self.tool('new', 'rust', invalid).returncode, 0)
        self.assertFalse(invalid.exists())

    def test_c_build_passes_all_sources_and_flags_without_shell_expansion(self):
        project = self.create()
        (project / 'src/extra file.c').write_text('int extra(void) { return 1; }\n')
        self.driver('aarch64', 'cc', '''import json, pathlib, sys
args = sys.argv[1:]
pathlib.Path('driver.json').write_text(json.dumps(args))
pathlib.Path(args[args.index('-o') + 1]).write_bytes(b'executable')
''')
        built = self.tool('build', '--arch', 'aarch64', cwd=project)
        self.assertEqual(built.returncode, 0, built.stderr)
        output = project / 'build/aarch64/example'
        self.assertEqual(built.stdout.splitlines()[-1], str(output))
        args = json.loads((project / 'driver.json').read_text())
        for flag in ('-std=c17', '-O2', '-Wall', '-Wextra'):
            self.assertIn(flag, args)
        self.assertIn(str(project / 'src/extra file.c'), args)
        self.assertIn(str(project / 'src/main.c'), args)
        self.assertEqual(output.read_bytes(), b'executable')

    def test_compiler_failure_is_not_an_old_success(self):
        project = self.create()
        old = project / 'build/aarch64/example'
        old.parent.mkdir(parents=True)
        old.write_bytes(b'old')
        self.driver('aarch64', 'cc', 'import sys\nsys.exit(23)\n')
        result = self.tool('build', '--arch', 'aarch64', cwd=project)
        self.assertEqual(result.returncode, 23)
        self.assertEqual(result.stdout, '')

    def test_rust_build_uses_named_json_artifact(self):
        project = self.create('rust-no-std')
        self.driver('aarch64', 'cargo', '''import json, pathlib, sys
assert sys.argv[1:] == ['build', '--release', '--offline', '--message-format=json-render-diagnostics']
for name in ('dependency', 'example'):
    output = pathlib.Path('target') / name
    output.parent.mkdir(exist_ok=True)
    output.write_bytes(name.encode())
    print(json.dumps({'reason': 'compiler-artifact', 'target': {'kind': ['bin'], 'name': name}, 'executable': str(output)}))
print(json.dumps({'reason': 'build-finished', 'success': True}))
''')
        result = self.tool('build', '--arch', 'aarch64', cwd=project)
        self.assertEqual(result.returncode, 0, result.stderr)
        output = project / 'build/aarch64/example'
        self.assertEqual(result.stdout.splitlines()[-1], str(output))
        self.assertEqual(output.read_bytes(), b'example')

    def test_cargo_failure_or_missing_artifact_never_reports_success(self):
        project = self.create('rust')
        for body in ('import sys\nsys.exit(19)\n', 'print(\'{"reason":"build-finished","success":true}\')\n'):
            self.driver('aarch64', 'cargo', body)
            result = self.tool('build', '--arch', 'aarch64', cwd=project)
            self.assertNotEqual(result.returncode, 0)
            self.assertEqual(result.stdout, '')

    def test_relocated_tools_and_symlink_entry_find_their_sdk(self):
        moved = self.work / 'relocated'
        shutil.move(self.sdk, moved)
        self.sdk = moved
        self.assertEqual(self.tool('new', '--version').stdout,
                         'breenix-sdk 0.1.0 new ' + 'a' * 40 + '\n')
        self.assertEqual(self.tool('build', '--version').stdout,
                         'breenix-sdk 0.1.0 build ' + 'a' * 40 + '\n')
        alias = self.work / 'new-alias'
        alias.symlink_to(moved / 'bin/breenix-new')
        result = subprocess.run([str(alias), 'c', 'relocated-app'], cwd=self.work, capture_output=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue((self.work / 'relocated-app/src/main.c').exists())


if __name__ == '__main__':
    unittest.main()
