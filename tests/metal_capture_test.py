"""Exercise SOL capture before PTY transformations, using a local producer."""
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time
import unittest
import uuid
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
import metal_tools_test as existing


class CaptureTest(unittest.TestCase):
    def test_binary_output_is_archived_before_the_terminal_and_respawns_append(self):
        with tempfile.TemporaryDirectory(prefix='metal-capture-') as temporary:
            root = Path(temporary)
            expected = b'\x00\x1b[31mSUITE signals CASE masks/block-pending PASS ms=0\x1b[0m\r\n' * 1024
            (root / 'payload').write_bytes(expected)
            producer = root / 'metal-sol'
            producer.write_text('''#!/usr/bin/env python3
import json, os, stat
from pathlib import Path
root = Path(__file__).parent
(root / 'descriptors').write_text(json.dumps([os.isatty(0), stat.S_ISFIFO(os.fstat(1).st_mode)]))
# Include bytes the terminal would transform, plus a burst larger than its queue.
with (root / 'payload').open('rb') as source:
    while data := source.read(4096):
        while data:
            data = data[os.write(1, data):]
''')
            producer.chmod(0o755)
            driver = object.__new__(existing.boot.Driver)
            driver.dir = root
            driver.tmux = ['tmux', '-L', 'metal-capture-' + uuid.uuid4().hex]
            server_pid = None
            try:
                with patch.object(existing.m, 'TOOLS', root):
                    for attempt in (0, 1):
                        driver.spawn_console(fresh=attempt == 0)
                        server_pid = int(subprocess.check_output(driver.tmux +
                            ['display-message', '-p', '-t', 'sol', '#{pid}']))
                        deadline = time.monotonic() + 5
                        while time.monotonic() < deadline and not driver.console_dead():
                            time.sleep(0.02)
                        self.assertTrue(driver.console_dead())
                        self.assertEqual((root / 'serial.raw').read_bytes(), expected * (attempt + 1))
                        self.assertEqual(json.loads((root / 'descriptors').read_text()), [True, True])
            finally:
                if server_pid is not None:
                    os.kill(server_pid, signal.SIGTERM)


if __name__ == '__main__':
    unittest.main()
