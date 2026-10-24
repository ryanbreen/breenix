#!/usr/bin/env python3
"""Exercise serial wakeup and the private QMP session without booting a VM."""
import importlib.util
import json
from pathlib import Path
import select
import socket
import tempfile
import threading
from types import SimpleNamespace
import unittest
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location('hostkeys', Path(__file__).resolve().parents[1] / 'scripts/hostkeys.py')
hostkeys = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(hostkeys)


class HostInputWatch(unittest.TestCase):
    @unittest.skipUnless(hasattr(select, 'kqueue'), 'native macOS/BSD file notifications')
    def test_append_between_eof_and_wait_delivers_pointer_without_poll_sleep(self):
        with tempfile.TemporaryDirectory() as folder:
            serial = Path(folder) / 'serial'
            serial.touch()
            delivered = []

            class Keys:
                def pointer(self, *args):
                    delivered.append(args)

            class AppendingQueue:
                def __init__(self):
                    self.queue = select.kqueue()
                    self.appended = False

                def control(self, changes, maximum, timeout):
                    if changes is None and not self.appended:
                        self.appended = True
                        serial.write_text('SUITE window-manager HOSTPOINTER startup/empty-desktop '
                                          'seq=1 x=2559 y=1599 width=2560 height=1600 buttons=0 wheel=0\n')
                    return self.queue.control(changes, maximum, timeout)

                def close(self):
                    self.queue.close()

            def parent_alive(*_):
                if delivered:
                    raise ProcessLookupError

            native = SimpleNamespace(kqueue=AppendingQueue, kevent=select.kevent,
                                     KQ_FILTER_VNODE=select.KQ_FILTER_VNODE,
                                     KQ_EV_ADD=select.KQ_EV_ADD, KQ_EV_CLEAR=select.KQ_EV_CLEAR,
                                     KQ_NOTE_WRITE=select.KQ_NOTE_WRITE)
            args = SimpleNamespace(backend='none', qmp_persistent=False, log=None, suite='window-manager',
                                   serial=str(serial), parent=1)
            with patch.object(hostkeys, 'select', native, create=True), \
                 patch.object(hostkeys, 'backend', return_value=Keys()), \
                 patch.object(hostkeys.os, 'kill', side_effect=parent_alive), \
                 patch.object(hostkeys.time, 'sleep', side_effect=AssertionError('polled instead of waiting for append')):
                hostkeys.watch(args)
            self.assertEqual(delivered, [(2559, 1599, 0, 0, 'absolute')])

    def test_private_qmp_negotiates_before_requests_and_reuses_connection(self):
        with tempfile.TemporaryDirectory() as folder:
            path = str(Path(folder) / 'input.sock')
            commands, errors = [], []
            with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as server:
                server.bind(path)
                server.listen(1)
                server.settimeout(2)

                def serve():
                    try:
                        with server.accept()[0] as peer, peer.makefile('rw') as stream:
                            peer.settimeout(2)
                            stream.write('{"QMP": {}}\n')
                            stream.flush()
                            for line in stream:
                                commands.append(json.loads(line)['execute'])
                                stream.write('{"return": {}}\n')
                                stream.flush()
                    except Exception as error:
                        errors.append(error)

                thread = threading.Thread(target=serve)
                thread.start()
                keys = hostkeys.QMPKeys(path)
                keys.persistent = True
                try:
                    keys.connect()
                    self.assertEqual(commands, ['qmp_capabilities'])
                    keys.send((), 'a')
                    keys.pointer(2559, 1599, 0, 0, 'absolute', 2560, 1600)
                finally:
                    keys.close()
                    thread.join(3)
                self.assertFalse(thread.is_alive())
                self.assertEqual(errors, [])
                self.assertEqual(commands, ['qmp_capabilities', 'send-key', 'input-send-event'])


if __name__ == '__main__':
    unittest.main()
