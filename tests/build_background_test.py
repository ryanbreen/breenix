"""Execute the build launcher with real descriptors and, on Mac, policy readback."""
from pathlib import Path
import subprocess
import sys
import unittest

LAUNCHER = Path(__file__).resolve().parents[1] / 'scripts/build-background.sh'


class BuildBackgroundTest(unittest.TestCase):
    def test_command_io_and_exit_status_with_policy_readback_on_mac(self):
        probe = '''import ctypes, os, sys
if sys.platform == 'darwin':
    libc = ctypes.CDLL(None, use_errno=True)
    # getpriority(2): background readback is 1; PRIO_DARWIN_BG (0x1000)
    # is the setpriority input, not its readback. Measured with setpriority
    # followed by getpriority on this Mac: 0 before, 1 after, errno 0.
    ctypes.set_errno(0)
    background = libc.getpriority(4, 0)  # PRIO_DARWIN_PROCESS
    assert (background, ctypes.get_errno()) == (1, 0), 'CPU policy is not background'
    assert libc.getiopolicy_np(0, 2) == 4, 'background disk policy is not utility'
print(sys.stdin.read() + sys.argv[1], end='')
sys.exit(23)
'''
        result = subprocess.run([str(LAUNCHER), sys.executable, '-c', probe, 'argument with spaces'],
                                input='input:', capture_output=True, text=True, timeout=20)
        self.assertEqual(result.returncode, 23, result.stderr)
        self.assertEqual(result.stdout, 'input:argument with spaces')


if __name__ == '__main__':
    unittest.main()
