import argparse
from pathlib import Path
import runpy
import unittest
from unittest.mock import Mock


class FlashPathTest(unittest.TestCase):
    def test_kernel_flash_uses_ssh_unless_explicitly_in_recovery(self):
        runner = runpy.run_path(str(Path(__file__).resolve().parents[1] / 'scripts/metal/metal-run'))
        scope = runner['install_persistent'].__globals__
        for recovery, live_result, expected in [
                (False, 0, 'live'), (False, 1, 'live'), (True, 1, 'network')]:
            with self.subTest(recovery=recovery, live_result=live_result):
                live = Mock(return_value=live_result)
                network = Mock(return_value=17)
                scope.update(live_flash=live, boot=network)
                args = argparse.Namespace(sha=None, branch=None, rollback=False, inject=None,
                                          kernel_only=True, recovery=recovery, replace_manager=False)
                result = runner['install_persistent'](args)
                self.assertEqual(result, live_result if expected == 'live' else 17)
                self.assertEqual(live.call_count, int(expected == 'live'))
                self.assertEqual(network.call_count, int(expected == 'network'))


if __name__ == '__main__':
    unittest.main()
