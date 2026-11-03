"""Console input must not wait for the archive/scoring loop to drain."""
from pathlib import Path
import sys
import tempfile
import threading
import time
import unittest
from types import SimpleNamespace
from unittest.mock import Mock, patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
import metal_tools_test as existing
boot = existing.boot


class ReplayReader(unittest.TestCase):
    def test_raw_requests_are_served_without_pumping_the_archive(self):
        d = object.__new__(boot.Driver)
        d.args = SimpleNamespace(mode='suite', suite='console')
        d.kernel_started = True
        d.fatal_at = None
        d.hostkey_watcher = None
        d.hostkey_reader = None
        d.lost = False
        d.redact = lambda value: value
        delivered = threading.Event()
        d.replay_send = Mock(side_effect=lambda _: delivered.set())
        # Archiving an event may block; the reader must queue it instead.
        with tempfile.TemporaryDirectory() as tmp:
            d.dir = Path(tmp)
            raw = d.dir / 'serial.raw'
            raw.write_bytes(b'')
            with patch.object(boot.m, 'event') as archived:
                d.start_hostkey_reader()
                try:
                    with raw.open('ab') as f:
                        f.write(b'\r\nSUITE console HOSTKEY keys/caps-per-vt seq=1 keys=a\r\n')
                    self.assertTrue(delivered.wait(1), 'raw HOSTKEY waited for the archive loop')
                    self.assertEqual(d.replay_send.call_count, 1)
                    with raw.open('ab') as f:
                        f.write(b'SUITE console HOSTKEY keys/caps-per-vt seq=1 keys=a\n')
                    # The main loop has deliberately made no pump_serial call.
                    time.sleep(0.05)
                    self.assertEqual(d.replay_send.call_count, 1, 'request replayed twice')
                    self.assertFalse(archived.called, 'replay performed synchronous archive I/O')
                    d.checkpoint()
                    self.assertTrue(archived.called)
                finally:
                    d.stop_hostkey_reader()
                self.assertFalse(d.hostkey_reader.is_alive())
                before = d.replay_send.call_count
                with raw.open('ab') as f:
                    f.write(b'SUITE console HOSTKEY keys/caps-per-vt seq=2 keys=b\n')
                time.sleep(0.05)
                self.assertEqual(d.replay_send.call_count, before)

    def test_reader_errors_fail_the_boot(self):
        d = object.__new__(boot.Driver)
        d.args = SimpleNamespace(mode='suite', suite='console')
        d.kernel_started = True
        d.fatal_at = None
        d.hostkey_reader = None
        d.lost = False
        d.redact = lambda value: value
        with tempfile.TemporaryDirectory() as tmp:
            d.dir = Path(tmp)
            d.on_hostkey_line = Mock(side_effect=OSError('fixture reader failure'))
            (d.dir / 'serial.raw').write_bytes(b'SUITE console HOSTKEY keys/caps-per-vt seq=1 keys=a\n')
            d.start_hostkey_reader()
            try:
                d.hostkey_reader.join(1)
                with self.assertRaises(boot.Finish):
                    d.checkpoint()
            finally:
                d.stop_hostkey_reader()


if __name__ == '__main__':
    unittest.main()
