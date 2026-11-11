#!/usr/bin/env python3
"""Exercise desktop readiness using fixture serial output without a VM."""
import contextlib
import io
import json
from pathlib import Path
import runpy
import tempfile
import time
import unittest
from unittest.mock import Mock, patch

ROOT = Path(__file__).resolve().parents[1]
WATCH = runpy.run_path(str(ROOT / 'scripts/watch-qemu.py'))['watch']
CATALOG = json.loads((ROOT / 'docs/boot-path.json').read_text())
STAGES = [stage['marker'].split('|')[0] for milestone in CATALOG['milestones']
          if milestone['id'] in ('first-userspace', 'runtimes')
          for stage in milestone['stages']['desktop']]


class DesktopReadyTest(unittest.TestCase):
    def watch_serial(self, lines, enabled=True):
        with tempfile.TemporaryDirectory() as directory:
            serial = Path(directory) / 'serial'
            serial.write_text('\n'.join(lines) + '\n')
            elapsed = [0.0]
            def tick(seconds):
                elapsed[0] += seconds
            with patch.object(time, 'monotonic', side_effect=lambda: elapsed[0]), \
                 patch.object(time, 'sleep', side_effect=tick):
                return WATCH(Mock(poll=lambda: None), serial, 'desktop', '', 0.6, 1, 0,
                             Mock(), until_desktop_ready=enabled)

    def test_full_desktop_passes(self):
        self.assertEqual(self.watch_serial(STAGES), 'done')

    def test_missing_client_frame_times_out(self):
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            self.assertEqual(self.watch_serial([s for s in STAGES if '[bwm] composited client frame:' not in s]), 'deadline')
        self.assertIn('No desktop-ready within', output.getvalue())
        self.assertNotIn('No suite DONE', output.getvalue())

    def test_bwm_failure_fails(self):
        self.assertEqual(self.watch_serial(STAGES + ['[bwm] desktop fill check FAILED']), 'fatal')

    def test_panic_fails(self):
        self.assertEqual(self.watch_serial(['KERNEL PANIC: fixture']), 'fatal')

    def test_without_flag_still_idles(self):
        self.assertEqual(self.watch_serial(STAGES, enabled=False), 'idle')


if __name__ == '__main__':
    unittest.main()
