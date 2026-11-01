#!/usr/bin/env python3
import importlib.util
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'scripts' / 'metal'))
import metal_root

spec = importlib.util.spec_from_file_location('vm_site', ROOT / 'scripts' / 'vm_site.py')
vm_site = importlib.util.module_from_spec(spec)
spec.loader.exec_module(vm_site)

# Public test key and synthetic credentials, never used by a real account.
KEY = 'ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA'
VALUES = {
    'OPERATOR_USER': 'example', 'OPERATOR_UID': '1000', 'OPERATOR_PASSWORD': 'example-password',
    'OPERATOR_PASSWORD_HASH': '$6$salt$' + 'A' * 86, 'OPERATOR_SSH_PUBLIC_KEY': KEY,
    'CONSOLE_USER': 'console', 'CONSOLE_PASSWORD': 'console-password',
    'SNTP_SERVERS': '192.0.2.1', 'ROOT_PASSWORD_HASH': 'example-hash',
}

class VmSiteTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.site = self.root / 'site'
        self.run = self.root / 'run'
        self.run.mkdir()
        (self.run / 'ssh-port').write_text('34567\n')
        self.write_site(VALUES)

    def write_site(self, values):
        self.site.write_text(''.join(f'{k}={v}\n' for k, v in values.items()))
        self.site.chmod(0o600)

    def test_permissions_and_owner(self):
        self.site.chmod(0o644)
        with self.assertRaises(ValueError): vm_site.load_site(self.site)
        self.site.chmod(0o600)
        with patch.object(vm_site.os, 'getuid', return_value=os.getuid() + 1):
            with self.assertRaises(ValueError): vm_site.load_site(self.site)

    def test_missing_fields(self):
        for key in VALUES:
            with self.subTest(key=key):
                self.write_site({k: v for k, v in VALUES.items() if k != key})
                with self.assertRaises(ValueError): vm_site.load_site(self.site)

    def test_site_uses_run_port_and_guest_host(self):
        site = vm_site.site(self.run, self.site)
        self.assertEqual(site['SERVER_SSH_HOST'], '127.0.0.1')
        self.assertEqual(site['SERVER_SSH_PORT'], 34567)
        self.assertEqual(site['BOOT_URL'], 'http://10.0.2.2/')
        self.assertEqual(site['operatorKeys'], metal_root.operator_keys(VALUES))
        for key in VALUES:
            if key != 'OPERATOR_PASSWORD_HASH': self.assertEqual(site[key], VALUES[key])

    def test_account_command_matches_metal_root(self):
        config = self.root / '.config' / 'breenix'
        config.mkdir(parents=True)
        (config / 'vm-site').write_bytes(self.site.read_bytes())
        (config / 'vm-site').chmod(0o600)
        out = self.root / 'account'
        subprocess.run([sys.executable, str(ROOT / 'scripts' / 'vm_site.py'), 'operator-account', '--out', str(out)],
                       env=dict(os.environ, HOME=str(self.root)), check=True)
        self.assertEqual(out.read_bytes(), metal_root.operator_account(VALUES).encode())
        self.assertEqual(out.stat().st_mode & 0o777, 0o600)

if __name__ == '__main__':
    unittest.main()
