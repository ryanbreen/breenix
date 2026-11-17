"""Exercise root-key exclusion in a disposable VM's real ext2 image."""
import importlib.util
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'scripts'))
spec = importlib.util.spec_from_file_location('vm_image', ROOT / 'scripts/vm-image.py')
vm_image = importlib.util.module_from_spec(spec)
spec.loader.exec_module(vm_image)


class VmImageRootKeysTest(unittest.TestCase):
    def test_staging_excludes_test_root_keys_and_keeps_operator_key(self):
        mke2fs, debugfs = vm_image.vm_site.metal_root.require_e2fsprogs('mke2fs', 'debugfs')
        site = dict(OPERATOR_USER='operator', OPERATOR_UID='1234',
                    OPERATOR_PASSWORD_HASH='$6$fixture$' + 'A' * 86,
                    OPERATOR_SSH_PUBLIC_KEY='ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA',
                    CONSOLE_USER='root', ROOT_PASSWORD_HASH='$6$console$' + 'B' * 86,
                    SNTP_SERVERS='192.0.2.123')
        with tempfile.TemporaryDirectory() as scratch:
            base = Path(scratch)
            tree = base / 'tree'
            (tree / 'etc/breenix').mkdir(parents=True)
            (tree / 'etc/passwd').write_text('root:x:0:0:root:/root:/bin/bsh\n')
            (tree / 'etc/shadow').write_text('root:!:::::::\n')
            (tree / 'root/.ssh').mkdir(parents=True)
            for name in ('authorized_keys', 'authorized_keys2'):
                (tree / 'root/.ssh' / name).write_text('test image key\n')
            account = base / 'account'
            account.write_text(vm_image.vm_site.metal_root.operator_account(site))
            image = base / 'image'
            with image.open('wb') as output:
                output.truncate(4 * 1024 * 1024)
            subprocess.run([mke2fs, '-q', '-t', 'ext2', '-F', '-d', str(tree), str(image)],
                           check=True, capture_output=True, timeout=10)
            with patch.object(vm_image.vm_site, 'load_site', return_value=site), \
                 patch.dict(os.environ, PATH=str(Path(debugfs).parent) + os.pathsep + os.environ.get('PATH', '')):
                vm_image.stage(str(image), account)
            for name in ('authorized_keys', 'authorized_keys2'):
                result = subprocess.run([debugfs, '-R', 'stat /root/.ssh/' + name, str(image)],
                                        capture_output=True, text=True, timeout=10)
                self.assertNotIn('Inode:', result.stdout)
                self.assertIn('File not found', result.stderr)
            result = subprocess.run(
                [debugfs, '-R', 'cat /.breenix-shared/var/lib/accounts/pending/operator', str(image)],
                check=True, capture_output=True, text=True, timeout=10)
            self.assertEqual(result.stdout, site['OPERATOR_SSH_PUBLIC_KEY'] + '\n')


if __name__ == '__main__':
    unittest.main()
