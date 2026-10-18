"""Exercise scoped Fusion inventory pruning and its running-app guard."""
import importlib.util
from pathlib import Path
import tempfile
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('vmware_inventory', ROOT / 'scripts/vmware-inventory.py')
inventory = importlib.util.module_from_spec(spec)
spec.loader.exec_module(inventory)


class VmwareInventoryTest(unittest.TestCase):
    def test_fixture_keeps_live_and_unrelated_entries_and_renumbers_both_groups(self):
        with tempfile.TemporaryDirectory() as temporary:
            machines = Path(temporary) / 'Virtual Machines.localized'
            live = machines / 'breenix-live.vmwarevm/boot.vmx'
            live.parent.mkdir(parents=True)
            live.touch()
            source = (ROOT / 'tests/fixtures/vmware/vmInventory').read_text().replace('@MACHINES@', str(machines))
            pruned = inventory.prune_inventory(source, machines)
            fields = {}
            for line in pruned.splitlines():
                match = inventory.GROUP.match(line)
                if match:
                    fields.setdefault((match[1], int(match[2])), {})[match[3]] = match[4]
            expected = [('breenix-live.vmwarevm', 'boot.vmx'), ('Unrelated.vmwarevm', 'guest.vmx'),
                        ('breenix.vmwarevm', 'breenix.vmx')]
            for number, (bundle, vmx) in enumerate(expected):
                self.assertEqual(fields[('vmlist', number + 1)]['config'], str(machines / bundle / vmx))
                self.assertEqual(fields[('index', number)]['id'], str(machines / bundle / vmx))
            self.assertEqual(len(fields), 6)
            self.assertEqual(fields[('vmlist', 2)]['ItemID'], '3')
            self.assertEqual(fields[('index', 1)]['field0.value'], 'Keep this field')
            self.assertIn('.encoding = "UTF-8"\n', pruned)
            self.assertEqual(pruned.count('index.count = "3"'), 1)
            self.assertNotIn('breenix-dead', pruned)
            self.assertEqual(inventory.prune_inventory(pruned, machines), pruned)

    def test_paths_outside_runner_bundles_are_untouched(self):
        machines = Path('/example/Virtual Machines.localized')
        for path in (str(machines) + '-other/breenix-dead/boot.vmx',
                     str(machines / 'breenix-dead/../../other/boot.vmx'),
                     str(machines / 'breenix-dead/notes.txt')):
            with self.subTest(path=path):
                source = f'vmlist7.config = "{path}"\nindex9.id = "{path}"\nindex.count = "1"\n'
                self.assertEqual(inventory.prune_inventory(source, machines), source)

    def test_file_is_only_replaced_while_fusion_stays_closed(self):
        for running in ([True], [False, True], [False, False]):
            with self.subTest(running=running), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                path = root / 'vmInventory'
                source = f'vmlist2.config = "{root}/breenix-dead/boot.vmx"\nindex4.id = "{root}/breenix-dead/boot.vmx"\nindex.count = "1"\n'
                path.write_text(source)
                path.chmod(0o640)
                with mock.patch.object(inventory, 'fusion_running', side_effect=running):
                    changed = inventory.prune_file(path, root)
                self.assertEqual(changed, running == [False, False])
                self.assertEqual(path.read_text(), 'index.count = "0"\n' if changed else source)
                self.assertEqual(path.stat().st_mode & 0o777, 0o640)
                self.assertEqual(list(root.iterdir()), [path])


if __name__ == '__main__':
    unittest.main()
