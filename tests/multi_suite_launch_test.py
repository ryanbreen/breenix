"""Exercise the one-case disk staging path and the lifetime of multi-suite registrations."""
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import unittest
from vigil_record_fixture import isolated_commands

ROOT = Path(__file__).resolve().parents[1]


class LaunchTest(unittest.TestCase):
    def test_one_case_vm_staging_writes_no_sequence(self):
        functions = dict(re.findall(r'(?ms)^(\w+)\(\) \{\n(.*?)^\}', (ROOT / 'run.sh').read_text()))
        names = ('writes_boot_target', 'target_nameserver', 'stage_boot_ext2_disk')
        definitions = '\n'.join(f'{name}() {{\n{functions[name]}\n}}' for name in names)
        tools = '/opt/homebrew/opt/e2fsprogs/sbin'
        env = dict(os.environ, PATH=tools + ':' + os.environ['PATH'])
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / 'scripts').mkdir()
            for script in ('write-boot-target.sh', 'write-resolv-conf.sh'):
                (root / 'scripts' / script).symlink_to(ROOT / 'scripts' / script)
            binaries = root / 'userspace/programs/aarch64'
            binaries.mkdir(parents=True)
            (binaries / 'suite-smoke.elf').write_bytes(b'fixture ELF')
            files = root / 'files'
            (files / 'sbin').mkdir(parents=True)
            (files / 'etc').mkdir()
            shutil.copy(binaries / 'suite-smoke.elf', files / 'sbin/suite-smoke')
            image = root / 'disk.img'
            image.write_bytes(bytes(4 * 1024 * 1024))
            subprocess.run(['mke2fs', '-q', '-F', '-t', 'ext2', '-d', str(files), str(image)], env=env, check=True, capture_output=True)
            for platform in ('PARALLELS', 'VMWARE'):
                run_env = dict(env, BREENIX_ROOT=str(root), BOOT_MODE='suite', SUITE='smoke', ONLY='basic/hello',
                               PROBE='false', PARALLELS=str(platform == 'PARALLELS').lower(), VMWARE=str(platform == 'VMWARE').lower())
                result = subprocess.run(['bash', '-c', definitions + '\nstage_boot_ext2_disk "$1" "$2"',
                                         'stage', str(image), str(root / platform)], env=run_env, capture_output=True, text=True)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                target = subprocess.check_output(['debugfs', '-R', 'cat /etc/breenix/boot-target', str(root / platform / 'ext2-boot-target.img')], env=env, stderr=subprocess.DEVNULL)
                self.assertEqual(target, b'suite smoke case basic/hello\n')

    def test_list_registration_keeps_the_live_launcher_pid(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            cli = root / 'vigil-cli'
            cli.write_text('''#!/bin/sh
if [ "$#" = 1 ]; then echo 'breenix start --cpus --suite a,b,c'; exit 0; fi
printf '%s\\n' "$@" >> "$CAPTURE"
echo boot
''')
            cli.chmod(0o755)
            env = {k: v for k, v in os.environ.items() if k not in ('VIGIL_BOOT_ID', 'BREENIX_LAUNCHER_PID')}
            env.update(VIGIL_CLI=str(cli), CAPTURE=str(root / 'args'))
            isolated_commands(env, root)
            script = 'echo $$ > "$1"; ids=$("$2" start qemu suite smoke,crypto "$3"); echo "$ids"; read -r done'
            launcher = subprocess.Popen(['bash', '-c', script, 'launcher', str(root / 'pid'), str(ROOT / 'scripts/vigil-record.sh'), str(root / 'serial')],
                                        env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
            try:
                self.assertEqual(launcher.stdout.readline().strip(), 'boot')
                args = (root / 'args').read_text().splitlines()
                self.assertEqual(args[args.index('--suite') + 1], 'smoke,crypto')
                self.assertEqual(args.count('--suite'), 1)
                pids = [int(args[i + 1]) for i, value in enumerate(args) if value == '--pid']
                self.assertEqual(pids, [launcher.pid])
                for pid in pids:
                    os.kill(pid, 0)
            finally:
                launcher.communicate('done\n', timeout=5)


if __name__ == '__main__':
    unittest.main()
