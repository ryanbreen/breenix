#!/usr/bin/env python3
"""Keep inherited stdin on the console's guarded read/wait path."""
from pathlib import Path
import unittest


class InheritedStdinTest(unittest.TestCase):
    def test_pid_one_stdin_uses_atomic_console_wait(self):
        source = (Path(__file__).resolve().parents[1] /
                  'kernel/src/syscall/handlers.rs').read_text()
        arm = source.split('FdKind::StdIo(0) => {', 1)[1].split('FdKind::StdIo(_) => {', 1)[0]
        # The installer inherits this descriptor. Register/read/block can lose
        # a wake between observing an empty ring and parking the reader.
        self.assertIn('super::blocking_io::read_console(', arm)
        self.assertNotIn('register_blocked_reader', arm)
        self.assertNotIn('block_current_in_syscall', arm)
        self.assertIn('status_flags::O_NONBLOCK', arm)
        self.assertLess(arm.index('drop(manager_guard)'), arm.index('read_console('))
        self.assertIn('adopt_foreground_pgrp_from_reader(reader_pid)', arm)


if __name__ == '__main__':
    unittest.main()
