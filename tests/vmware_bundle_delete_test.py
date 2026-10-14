"""Exercise run.sh's deletion of a stopped VMware VM bundle."""

import re
import subprocess
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]

# Fusion's vmrun refuses deleteVM while any lock in the bundle remains, as it does
# for the locks a killed vmware-vmx leaves behind.
FAKE_VMRUN = """#!/bin/sh
echo "$@" >> "$CALLS"
[ "$1" = deleteVM ] || exit 1
bundle=$(dirname "$2")
[ "$DELETE_FAILS" = 1 ] && { echo "Error: Unknown error"; exit 255; }
for lock in "$bundle"/*.lck; do
    [ -e "$lock" ] && { echo "Error: Insufficient permissions"; exit 255; }
done
rm -r "$bundle"
"""


class VmwareBundleDeleteTest(unittest.TestCase):
    def run_delete(self, root, bundle, delete_fails=False):
        functions = dict(re.findall(r'(?ms)^(\w+)\(\) \{\n(.*?)^\}', (ROOT / 'run.sh').read_text()))
        vmrun = root / 'vmrun'
        vmrun.write_text(FAKE_VMRUN)
        vmrun.chmod(0o755)
        script = ('bounded_vm_command() { "$@"; }\n'
                  f'delete_vmware_bundle() {{\n{functions["delete_vmware_bundle"]}}}\n'
                  f'VMRUN={vmrun}\ndelete_vmware_bundle "{bundle}"\n')
        env = {'PATH': '/usr/bin:/bin', 'CALLS': str(root / 'calls'),
               'DELETE_FAILS': '1' if delete_fails else '0'}
        return subprocess.run(['/bin/bash', '-c', script], env=env, capture_output=True,
                              text=True, timeout=10)

    def make_bundle(self, root, with_vmx=True):
        bundle = root / 'breenix-1700000000.vmwarevm'
        bundle.mkdir()
        (bundle / 'boot.vmdk').write_text('disk')
        if with_vmx:
            (bundle / 'breenix-1700000000.vmx').write_text('displayName = "x"\n')
            for lock in ('breenix-1700000000.vmx.lck', 'boot.vmdk.lck'):
                (bundle / lock).mkdir()
                (bundle / lock / 'M12345.lck').write_text('stale')
        return bundle

    def test_stale_locks_are_cleared_before_delete_vm(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            bundle = self.make_bundle(root)
            result = self.run_delete(root, bundle)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertFalse(bundle.exists(), result.stdout)
            self.assertEqual((root / 'calls').read_text().split(), ['deleteVM', str(bundle / 'breenix-1700000000.vmx')])

    def test_bundle_without_vmx_is_removed_without_vmrun(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            bundle = self.make_bundle(root, with_vmx=False)
            result = self.run_delete(root, bundle)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertFalse(bundle.exists())
            self.assertFalse((root / 'calls').exists())

    def test_failed_delete_vm_is_reported_and_bundle_kept(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            bundle = self.make_bundle(root)
            result = self.run_delete(root, bundle, delete_fails=True)
            self.assertIn(f'WARNING: vmrun deleteVM kept {bundle}: Error: Unknown error', result.stdout)
            self.assertTrue((bundle / 'breenix-1700000000.vmx').exists())


if __name__ == '__main__':
    unittest.main()
