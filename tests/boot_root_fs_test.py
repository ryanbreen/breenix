#!/usr/bin/env python3
"""Check the boot root choice without starting a VM."""
import subprocess
import os
import shutil
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


class BootRootFsTest(unittest.TestCase):
    def test_suite_disk_declares_the_root_choice(self):
        boot = (ROOT / 'scripts/boot-interactive.sh').read_text()
        # Run the launcher's disk-staging block in isolation.
        comment = boot.index('    # The first suite runs from fw_cfg;')
        start = boot.rfind('\nif ', 0, comment)
        stage = boot[start:boot.index('\nSMB_INSTANCE=', comment)]
        env = dict(os.environ, PATH='/opt/homebrew/opt/e2fsprogs/sbin:' + os.environ['PATH'])
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / 'scripts').mkdir()
            (root / 'scripts/write-boot-target.sh').symlink_to(ROOT / 'scripts/write-boot-target.sh')
            binaries = root / 'userspace/programs/aarch64'
            binaries.mkdir(parents=True)
            tree = root / 'tree'
            (tree / 'sbin').mkdir(parents=True)
            (tree / 'etc/breenix').mkdir(parents=True)
            for name in ('directories', 'filesystem', 'files-io'):
                binary = binaries / f'suite-{name}.elf'
                binary.write_bytes(b'fixture ELF')
                shutil.copy(binary, tree / 'sbin' / f'suite-{name}')
            image = root / 'root.img'
            for suites in ('filesystem', 'directories,filesystem,files-io'):
                for root_fs in ('ext2', 'breenfs'):
                    with self.subTest(suites=suites, root_fs=root_fs):
                        image.write_bytes(bytes(4 * 1024 * 1024))
                        subprocess.run(['mke2fs', '-q', '-F', '-t', 'ext2', '-d', str(tree), str(image)],
                                       env=env, capture_output=True, check=True)
                        result = subprocess.run(['bash', '-euc', stage], env=dict(env,
                            ROOT=str(root), WRITABLE=str(image), SUITE=suites, MODE='suite',
                            ROOT_FS=root_fs, ONLY='', PACE=''), text=True, capture_output=True)
                        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                        declaration = subprocess.run(['debugfs', '-R', 'cat /etc/breenix/root-fs', str(image)],
                                                     env=env, text=True, capture_output=True, check=True).stdout
                        if root_fs == 'breenfs':
                            self.assertEqual(declaration, 'root-fs breenfs\n')
                        else:
                            self.assertIn(declaration, ('', 'root-fs ext2\n'))

    def test_guest_rejects_unknown_declarations(self):
        with tempfile.TemporaryDirectory() as tmp:
            source = Path(tmp) / 'declaration.rs'
            source.write_text(f'''#[path = "{ROOT}/userspace/programs/src/filesystem_root.rs"]
mod filesystem_root;
use filesystem_root::{{parse, RootFs}};
fn main() {{
    let _ = filesystem_root::declared as fn() -> Result<RootFs, String>;
    assert_eq!(parse(None).unwrap(), RootFs::Ext2);
    for (name, kind, magic) in [("ext2", RootFs::Ext2, 0xef53),
        ("breenfs", RootFs::Breenfs, u64::from_le_bytes(*b"BREENFSS"))] {{
        let declaration = format!("root-fs {{name}}\\n");
        let parsed = parse(Some(&declaration)).unwrap();
        assert_eq!(parsed, kind);
        assert_eq!(parsed.name(), name);
        assert_eq!(parsed.magic(), magic);
    }}
    for invalid in ["root-fs unknown\\n", "root-fs\\n", "", "breenfs\\n",
        "root-fs breenfs extra\\n", "root-fs ext2\\nroot-fs breenfs\\n"] {{
        assert!(parse(Some(invalid)).is_err(), "accepted {{invalid:?}}");
    }}
}}
''')
            binary = Path(tmp) / 'declaration'
            subprocess.run(['taskpolicy', '-b', 'rustc', '--edition=2021', '-Dwarnings',
                            str(source), '-o', str(binary)], check=True)
            subprocess.run([str(binary)], check=True)

    def test_default_and_breenfs_drive_selection(self):
        with tempfile.TemporaryDirectory() as tmp:
            script = r'''
set -eu
ROOT=$1
WRITABLE=$2/root.img
MODE=suite
SUITE=directories,filesystem,files-io
run_build() { printf 'build:%s\n' "$*"; }
source "$ROOT/scripts/lib/root-fs.sh"
ROOT_FS=ext2
root_fs_prepare
printf 'ext2:%s\n' "$ROOT_DRIVE"
ROOT_FS=breenfs
root_fs_prepare
printf 'breenfs:%s\n' "$ROOT_DRIVE"
root_fs_scratch
printf 'scratch:%s\n' "${BREENFS_SCRATCH_ARGS[*]}"
'''
            result = subprocess.run(['bash', '-c', script, 'test', str(ROOT), tmp],
                                    text=True, capture_output=True, check=True).stdout
            self.assertIn(f'ext2:if=none,id=ext2,format=raw,file={tmp}/root.img,cache=unsafe,aio=threads', result)
            self.assertIn('scripts/create-breenfs-root.sh', result)
            boot = (ROOT / 'scripts/boot-interactive.sh').read_text()
            self.assertLess(boot.index('${BREENFS_SCRATCH_ARGS[@]'), boot.index('${BREENFS_READ_ARGS[@]'))
            self.assertIn('--root-fs', boot)

    def test_filesystem_scratch_drive_uses_the_shared_builder(self):
        boot = (ROOT / 'scripts/boot-interactive.sh').read_text()
        start = boot.index('\nBREENFS_CORRUPT_ARGS=()\n')
        end = boot.index('\nroot_fs_scratch\n', start) + len('\nroot_fs_scratch\n')
        stage = boot[start:end]
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            scripts = root / 'scripts'
            scripts.mkdir()
            for name in ('breenfs-suite-fixtures.sh', 'lib'):
                (scripts / name).symlink_to(ROOT / 'scripts' / name)
            for name in ('breenfs-read-fixture.sh', 'breenfs-corrupt-fixture.sh', 'filesystem-suite.py'):
                (scripts / name).touch()
            mkfs = root / 'tools/release/breenfs-mkfs'
            mkfs.parent.mkdir(parents=True)
            # Record the formatter's arguments in the very file QEMU will attach.
            mkfs.write_text('#!/bin/sh\n[ "$#" -eq 3 ] && printf "%s\\n" "$1" "$2" > "$3"\n')
            mkfs.chmod(0o755)

            run = 'run_build() { "$@"; }\n' + stage
            run += 'printf "%s\\n" "${BREENFS_SCRATCH_ARGS[@]}"\n'
            result = subprocess.run(['bash', '-euc', run], text=True, capture_output=True,
                                    env=dict(os.environ, ROOT=str(root), ROOT_FS='ext2',
                                             MODE='suite', SUITE='directories,filesystem,files-io',
                                             SERIAL_LOG=str(root / 'serial.log'),
                                             WRITABLE=str(root / 'root.img'),
                                             CARGO_TARGET_DIR=str(root / 'tools')))
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            scratch = root / 'breenfs-scratch.img'
            self.assertIn(f'if=none,id=breenfs-scratch,format=raw,file={scratch}', result.stdout.splitlines())
            self.assertEqual(scratch.read_text(), '--size\n67108864\n')


if __name__ == '__main__':
    unittest.main()
