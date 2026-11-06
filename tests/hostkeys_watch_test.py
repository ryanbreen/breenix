#!/usr/bin/env python3
"""Exercise serial wakeup and the host pointer transport without booting a VM."""
import importlib.util
from pathlib import Path
import select
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location('hostkeys', Path(__file__).resolve().parents[1] / 'scripts/hostkeys.py')
hostkeys = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(hostkeys)


class HostInputWatch(unittest.TestCase):
    def test_tagged_requests_do_not_kill_native_watchers(self):
        import inspect
        def record(method, sent):
            signature = inspect.signature(method)
            def call(*args, **kwargs):
                signature.bind(*args, **kwargs)
                sent.append(args)
            return call
        for cls in (hostkeys.QMPKeys, hostkeys.ParallelsKeys, hostkeys.VNCKeys, hostkeys.NoKeys):
            with self.subTest(backend=cls.__name__):
                keys, sent, logs = object.__new__(cls), [], []
                if cls is hostkeys.NoKeys:
                    keys.why = 'no backend'
                else:
                    keys.send = record(keys.send, sent)
                    keys.pointer = record(keys.pointer, sent)
                watcher = hostkeys.Watcher(keys, cls.__name__, logs.append)
                watcher.line('SUITE window-manager HOSTPOINTER lifecycle/close-request seq=1 '
                             'x=2520 y=1500 width=2560 height=1600 buttons=0 wheel=0 ticket=55')
                watcher.line('SUITE window-manager HOSTKEY focus/text-exact seq=2 keys=shift-h,e ticket=56')
                watcher.line('SUITE window-manager HOSTKEY focus/key seq=3 keys=a ticket=57')
                watcher.line('SUITE window-manager HOSTKEY focus/key seq=4 keys=b')
                self.assertEqual(len(logs), 4)
                if cls is not hostkeys.NoKeys:
                    self.assertEqual(len(sent), 5)

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
            self.assertEqual(delivered, [(2559, 1599, 0, 0, 'absolute', 2560, 1600)])

    def test_pointer_wire_and_guest_projection(self):
        fixture = (Path(__file__).parent / 'fixtures/bxptr-vectors.txt').read_text().splitlines()
        expected = [line.split(' | ')[1] for line in fixture if line and not line.startswith('#')]
        wire = []
        watcher = hostkeys.Watcher(hostkeys.ReplayKeys(wire.append), 'host-channel', lambda _: None)
        for seq, x, y in ((1, 0, 0), (2, 2559, 1599)):
            watcher.line(f'SUITE window-manager HOSTPOINTER edge/case seq={seq} x={x} y={y} '
                         'width=2560 height=1600 buttons=5 wheel=-1')
        self.assertEqual(wire, expected[:2])
        for line, point in zip(wire, ((0, 0), (2559, 1599))):
            x, y, width, height, _, _ = map(int, line.split()[2:])
            values = [(pixel * 32767 + size - 2) // (size - 1)
                      for pixel, size in ((x, width), (y, height))]
            self.assertEqual(tuple(value * (size - 1) // 32767
                                   for value, size in zip(values, (width, height))), point)
        mismatch = []
        other = hostkeys.Watcher(hostkeys.ReplayKeys(mismatch.append), 'host-channel', lambda _: None)
        other.line('SUITE window-manager HOSTPOINTER edge/case seq=1 x=0 y=0 '
                   'width=1280 height=800 buttons=5 wheel=-1')
        self.assertEqual(mismatch, [expected[4]])
        self.assertNotEqual(tuple(map(int, mismatch[0].split()[4:6])), (2560, 1600))

    def test_every_backend_accepts_pointer_dimensions(self):
        import inspect
        for cls in (hostkeys.QMPKeys, hostkeys.ParallelsKeys, hostkeys.VNCKeys, hostkeys.ReplayKeys):
            with self.subTest(backend=cls.__name__):
                inspect.signature(cls.pointer).bind(None, 1, 2, 0, 0, 'absolute', 2560, 1600)


if __name__ == '__main__':
    unittest.main()
