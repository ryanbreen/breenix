"""QEMU GUI staging matches the corresponding metal root files."""
import hashlib
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import unittest

import metal_smb_test
import metal_root
import vm_site

ROOT = Path(__file__).resolve().parents[1]


class DesktopStageTest(unittest.TestCase):
    def test_gui_selection_matches_metal_without_touching_base(self):
        debugfs, = metal_root.require_e2fsprogs('debugfs')
        site = {key: 'fixture' for key in vm_site.REQUIRED}
        site['SNTP_SERVERS'] = 'clock.example 192.0.2.1'
        for case in (None, 'guskit', 'guskit/smb-save-open', 'bterm/launch'):
            with self.subTest(case=case), metal_smb_test.SmbStagingTest().gui_root() as (base, _, config):
                original = hashlib.sha256(base.read_bytes()).digest()
                qemu = base.with_name('qemu.img')
                metal = base.with_name('metal.img')
                shutil.copyfile(base, qemu)
                shutil.copyfile(base, metal)
                site_file = base.with_name('site')
                site_file.write_text(''.join(f'{key}={value}\n' for key, value in site.items()))
                site_file.chmod(0o600)
                metal_root.stage_root(metal, 'suite', 'gui-apps', site, smb_config=config, case=case)
                subprocess.run(['python3', str(ROOT / 'scripts/qemu-desktop-stage.py'), str(qemu),
                                'gui-apps', case or ''], check=True,
                               env=dict(os.environ, BREENIX_VM_SITE=str(site_file)))
                def read(image, command):
                    return subprocess.run([debugfs, '-R', command, str(image)], capture_output=True, check=True)
                for name in ('desktop-suite', 'sntp-servers'):
                    path = '/etc/breenix/' + name
                    if case and case.startswith('bterm/'):
                        self.assertIn(b'File not found', read(qemu, 'stat ' + path).stderr)
                    else:
                        self.assertEqual(read(qemu, 'cat ' + path).stdout, read(metal, 'cat ' + path).stdout)
                        def metadata(image):
                            stat = read(image, 'stat ' + path).stdout.decode()
                            return re.search(r'Mode:\s+(\d+).*User:\s+(\d+)\s+Group:\s+(\d+)', stat).groups()
                        self.assertEqual(metadata(qemu), metadata(metal))
                self.assertEqual(hashlib.sha256(base.read_bytes()).digest(), original)


if __name__ == '__main__':
    unittest.main()
