"""Check failed Kbuild and final-link diagnostics without cross compilation."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class BusyboxBuildTests(unittest.TestCase):
    def run_build(self, compile_fails):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            scripts = root / 'scripts'
            scripts.mkdir()
            shutil.copy(ROOT / 'scripts/build-libc-busybox.sh', scripts)
            vendor = root / 'vendor/busybox'
            vendor.mkdir(parents=True)
            (vendor / 'busybox-1.37.0.tar.bz2').symlink_to(ROOT / 'vendor/busybox/busybox-1.37.0.tar.bz2')
            (vendor / 'breenix-libc.config').write_text('CONFIG_CAT=y\nCONFIG_ASH=y\n')
            (vendor / 'required-applets').write_text('cat\nash\n')
            (vendor / 'patches').mkdir()
            (vendor / 'patches/series').write_text('')
            (root / 'libs/libbreenix-libc/include').mkdir(parents=True)
            libc = root / 'libs/libbreenix-libc/target/aarch64-breenix/release/libc.a'
            libc.parent.mkdir(parents=True)
            libc.write_bytes(b'fixture archive')
            tools = root / 'tools'
            tools.mkdir()

            def executable(path, text):
                path.write_text('#!/bin/sh\nset -eu\n' + text)
                path.chmod(0o755)

            executable(tools / 'clang', 'echo "fixture compiler"\n')
            executable(tools / 'llvm-ar', 'touch "$2"\n')
            executable(scripts / 'install-libc-busybox.sh', 'exit 0\n')
            executable(scripts / 'breenix-cc', f'''
case "$*" in
  *--tool-dir*) echo '{tools}'; exit 0 ;;
  *-Wl,-e,*) exit 0 ;;
  *) echo 'deliberate final link error' >&2; exit 1 ;;
esac
''')
            executable(tools / 'make', '''
cd "$2"
case "$*" in
  *allnoconfig*) printf 'CONFIG_CAT=y\nCONFIG_ASH=y\n' > .config ;;
  *applets/built-in.o*)
    printf '\177ELFfixture' > coreutils/cat.o
    if [ "$COMPILE_FAILS" = 1 ]; then
      echo 'make: *** [shell/ash.o] Error 1' >&2
      exit 1
    fi
    printf '\177ELFfixture' > shell/ash.o ;;
esac
''')
            environment = dict(os.environ, PATH=str(tools) + ':' + os.environ['PATH'],
                               COMPILE_FAILS=str(int(compile_fails)), CARGO_BUILD_JOBS='4')
            result = subprocess.run(['bash', str(scripts / 'build-libc-busybox.sh'), '--arch', 'aarch64'],
                                    capture_output=True, text=True, env=environment)
            report = json.loads((root / 'target/libc-busybox/aarch64/applets.json').read_text())
            self.assertNotEqual(result.returncode, 0)
            self.assertFalse((root / 'userspace/programs/aarch64/libc-busybox.elf').exists())
            return result, report

    def test_compile_failure_preserves_each_applet_result(self):
        result, report = self.run_build(True)
        self.assertFalse(report['compile_ok'])
        self.assertTrue(report['applets']['cat']['compiles'])
        self.assertTrue(report['applets']['cat']['links'])
        self.assertFalse(report['applets']['ash']['compiles'])
        self.assertIn('compilation failed', result.stderr)
        self.assertNotIn('unresolved core:', result.stderr)

    def test_final_link_failure_prints_linker_diagnostic(self):
        result, report = self.run_build(False)
        self.assertTrue(report['compile_ok'])
        self.assertTrue(all(a['compiles'] and a['links'] for a in report['applets'].values()))
        self.assertIn('final link failed:', result.stderr)
        self.assertIn('deliberate final link error', result.stderr)


if __name__ == '__main__':
    unittest.main()
