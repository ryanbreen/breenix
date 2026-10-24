"""Exercise suite pointer requests through the real QMP encoder and verdict parser."""
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, ROOT / path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


HOST = load('e18_hostkeys', 'scripts/hostkeys.py')
VERDICT = load('e18_verdict', 'scripts/suite-verdict.py')


class Socket:
    def __init__(self):
        self.commands = []
        self.lines = iter(['{"QMP":{}}\n', '{"return":{}}\n', '{"return":{}}\n'])

    def __enter__(self):
        return self

    def __exit__(self, *args):
        pass

    def settimeout(self, value):
        pass

    def connect(self, path):
        pass

    def makefile(self, mode):
        return self

    def write(self, text):
        self.commands.append(json.loads(text))

    def flush(self):
        pass

    def readline(self):
        return next(self.lines)


class PointerTests(unittest.TestCase):
    def test_qmp_extents_buttons_and_wheel_release(self):
        socket = Socket()
        with patch.object(HOST.socket, 'socket', return_value=socket):
            HOST.QMPKeys('fixture').pointer(2559, 1599, 6, -1, 'absolute', 2560, 1600)
        self.assertEqual(socket.commands, [
            {'execute': 'qmp_capabilities', 'arguments': {}},
            {'execute': 'input-send-event', 'arguments': {'events': [
                {'type': 'abs', 'data': {'axis': 'x', 'value': 32767}},
                {'type': 'abs', 'data': {'axis': 'y', 'value': 32767}},
                {'type': 'btn', 'data': {'button': 'left', 'down': False}},
                {'type': 'btn', 'data': {'button': 'right', 'down': True}},
                {'type': 'btn', 'data': {'button': 'middle', 'down': True}},
                {'type': 'btn', 'data': {'button': 'wheel-down', 'down': True}},
                {'type': 'btn', 'data': {'button': 'wheel-down', 'down': False}},
            ]}},
        ])

    def test_watcher_deduplicates_and_refuses_outside_coordinates(self):
        socket = Socket()
        logs = []
        watcher = HOST.Watcher(HOST.QMPKeys('fixture'), 'qmp', logs.append)
        request = ('SUITE window-manager HOSTPOINTER pointer/capture seq=7 '
                   'x=0 y=0 width=2560 height=1600 buttons=1 wheel=0\n')
        with patch.object(HOST.socket, 'socket', return_value=socket):
            watcher.feed(request + request + request.replace('seq=7', 'seq=8').replace('x=0', 'x=2560'))
        self.assertEqual(len(socket.commands), 2)
        self.assertIn('outside declared mode', logs[-1])

    def test_pointer_record_is_transport_and_cannot_replace_a_case(self):
        fixtures = ROOT / 'tests/fixtures/suite-verdict'
        manifest = json.loads((fixtures / 'manifest.json').read_text())
        text = (fixtures / 'all-pass.txt').read_text()
        request = ('SUITE fixture HOSTPOINTER first/one seq=1 '
                   'x=0 y=0 width=2560 height=1600 buttons=1 wheel=0\n')
        with tempfile.TemporaryDirectory() as directory:
            serial = Path(directory) / 'serial'
            with_request = text.replace('SUITE fixture CASE first/one', request + 'SUITE fixture CASE first/one')
            serial.write_text(with_request)
            self.assertTrue(VERDICT.verdict(manifest, serial, [serial])[0])
            serial.write_text(with_request.replace('SUITE fixture CASE first/one PASS ms=1\n', ''))
            self.assertFalse(VERDICT.verdict(manifest, serial, [serial])[0])


if __name__ == '__main__':
    unittest.main()
