"""The upgrade driver must not read the previous boot's forwarded port."""
from pathlib import Path
import runpy
import tempfile
import unittest
from unittest.mock import Mock, patch


class LaunchReadyTest(unittest.TestCase):
    def test_boot_waits_for_live_lease_after_record_is_published(self):
        module = runpy.run_path(str(Path(__file__).resolve().parents[1] / 'scripts/metal/server_upgrade_vm.py'))
        with tempfile.TemporaryDirectory() as directory:
            run = Path(directory)
            (run / 'current').write_text('old')
            (run / 'mode').write_text('console')
            machine = Mock(run=run)
            successor = Mock()
            successor.boot = 'new'
            successor.holds_lease.side_effect = [False, True]
            successor.ended.return_value = False
            machine.select.return_value = successor
            child = Mock()
            child.poll.return_value = None
            def launch(*args, **kwargs):
                (run / 'current').write_text('new')
                return child
            with patch.object(module['subprocess'], 'Popen', side_effect=launch), \
                 patch.object(module['time'], 'sleep'):
                result = module['VmOps'](machine, 'a' * 40).boot('next')
            self.assertIs(result, successor)
            self.assertEqual(successor.holds_lease.call_count, 2)


if __name__ == '__main__':
    unittest.main()
