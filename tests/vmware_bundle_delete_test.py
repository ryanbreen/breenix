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
if [ "$1" = list ]; then printf 'Total running VMs: 0\n'; [ -z "$RUNNING" ] || printf '%s\n' "$RUNNING"; exit 0; fi
[ "$1" = deleteVM ] || exit 1
bundle=$(dirname "$2")
[ "$DELETE_FAILS" = 1 ] && { echo "Error: Unknown error"; exit 255; }
for lock in "$bundle"/*.lck; do
    [ -e "$lock" ] && { echo "Error: Insufficient permissions"; exit 255; }
done
# Fusion deletes known VM files, retaining any runner metadata.
for item in "$bundle"/*.vmx "$bundle"/*.vmdk; do rm -f "$item"; done
"""


class VmwareBundleDeleteTest(unittest.TestCase):
    def run_delete(self, root, bundle, delete_fails=False, owned=True, running=""):
        functions = dict(re.findall(r'(?ms)^(\w+)\(\) \{\n(.*?)^\}', (ROOT / 'run.sh').read_text()))
        vmrun = root / 'vmrun'
        vmrun.write_text(FAKE_VMRUN)
        vmrun.chmod(0o755)
        script = ('set -e\n'
                  'bounded_vm_command() { "$@"; }\n'
                  f'mktemp() {{ mkdir "{root}/scratch"; printf \"%s\\n\" \"{root}/scratch\"; }}\n'
                  f'vmware_bundle_stopped() {{\n{functions["vmware_bundle_stopped"]}}}\n'
                  f'delete_vmware_bundle() {{\n{functions["delete_vmware_bundle"]}}}\n'
                  f'VMRUN={vmrun}\nVM_BUNDLE="{bundle}"\nVM_NAME="{bundle.stem}"\nVM_BUNDLE_OWNED={str(owned).lower()}\ndelete_vmware_bundle "{bundle}"\necho continued\n')
        env = {'PATH': '/usr/bin:/bin', 'CALLS': str(root / 'calls'),
               'DELETE_FAILS': '1' if delete_fails else '0', 'RUNNING': running}
        return subprocess.run(['/bin/bash', '-c', script], env=env, capture_output=True,
                              text=True, timeout=10)

    def make_bundle(self, root, with_vmx=True):
        bundle = root / 'breenix-1700000000.vmwarevm'
        bundle.mkdir()
        (bundle / 'boot.vmdk').write_text('disk')
        (bundle / '.breenix-run-owner').write_text(bundle.stem + '\n')
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
            self.assertEqual((root / 'calls').read_text().split(), ['list', 'deleteVM', str(bundle / 'breenix-1700000000.vmx')])

    def test_bundle_without_vmx_is_removed_without_vmrun(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            bundle = self.make_bundle(root, with_vmx=False)
            result = self.run_delete(root, bundle)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertFalse(bundle.exists())
            self.assertEqual((root / 'calls').read_text().strip(), 'list')

    def test_unknown_contents_without_vmx_are_kept(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            bundle = self.make_bundle(root, with_vmx=False)
            unknown = bundle / 'important.txt'
            unknown.write_text('keep')
            result = self.run_delete(root, bundle)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(unknown.read_text(), 'keep')
            self.assertFalse((bundle / 'boot.vmdk').exists())

    def test_stale_owned_slot_bundle_is_deleted_before_recreation(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            bundle = self.make_bundle(root)
            functions = dict(re.findall(r'(?ms)^(\w+)\(\) \{\n(.*?)^\}', (ROOT / 'run.sh').read_text()))
            block = (ROOT / 'run.sh').read_text().split('    VM_BUNDLE_OWNED=false', 1)[1].split('    # Copy VMDKs', 1)[0]
            vmrun = root / 'vmrun'
            vmrun.write_text(FAKE_VMRUN)
            vmrun.chmod(0o755)
            script = ('set -e\nbounded_vm_command() { "$@"; }\n'
                      'cleanup_vmware_bundle() { :; }\n'
                      f'vmware_bundle_stopped() {{\n{functions["vmware_bundle_stopped"]}}}\n'
                      f'delete_vmware_bundle() {{\n{functions["delete_vmware_bundle"]}}}\n'
                      f'VMRUN="{vmrun}"\nVM_BUNDLE="{bundle}"\nVM_NAME="{bundle.stem}"\nVM_BUNDLE_OWNED=false\n{block}')
            result = subprocess.run(['/bin/bash', '-c', script], capture_output=True, text=True,
                                    env={'PATH': '/usr/bin:/bin', 'CALLS': str(root / 'calls')}, timeout=10)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertEqual(sorted(p.name for p in bundle.iterdir()), ['.breenix-run-owner'])
            self.assertIn('deleteVM', (root / 'calls').read_text())

    def test_failed_delete_vm_is_reported_and_bundle_kept(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            bundle = self.make_bundle(root)
            result = self.run_delete(root, bundle, delete_fails=True)
            self.assertIn(f'WARNING: vmrun deleteVM kept {bundle}: Error: Unknown error', result.stdout)
            self.assertTrue((bundle / 'breenix-1700000000.vmx').exists())
            self.assertEqual(result.returncode, 0)
            self.assertIn('continued', result.stdout)
            self.assertFalse((bundle / 'boot.vmdk.lck').exists())
            self.assertFalse((root / 'scratch').exists())


    def test_unowned_or_running_bundle_is_never_removed(self):
        for owned, running in [(False, False), (True, True)]:
            with self.subTest(owned=owned), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                bundle = self.make_bundle(root, with_vmx=False)
                other_vmx = bundle / 'Ubuntu.vmx'
                other_vmx.write_text('unrelated VM')
                result = self.run_delete(root, bundle, owned=owned,
                                         running=str(other_vmx) if running else '')
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertTrue(other_vmx.exists())

    def test_creation_collision_cannot_install_exit_trap(self):
        source = (ROOT / 'run.sh').read_text()
        block = source.split('    VM_BUNDLE_OWNED=false', 1)[1].split('    # Copy VMDKs', 1)[0]
        with tempfile.TemporaryDirectory() as temporary:
            bundle = Path(temporary) / 'existing.vmwarevm'
            bundle.mkdir()
            original = bundle / 'existing.vmx'
            original.write_text('unrelated VM')
            script = f'set -e\nVM_BUNDLE="{bundle}"\nVM_NAME=existing\nVM_BUNDLE_OWNED=false\n{block}'
            result = subprocess.run(['/bin/bash', '-c', script], capture_output=True, text=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertEqual(original.read_text(), 'unrelated VM')
            self.assertFalse((bundle / '.breenix-run-owner').exists())


if __name__ == '__main__':
    unittest.main()
