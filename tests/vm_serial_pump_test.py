#!/usr/bin/env python3
import json
import socket
import subprocess
import tempfile
import time
import unittest
from pathlib import Path

PUMP = Path(__file__).resolve().parents[1] / 'scripts/vm-serial-pump.py'


def connect(path, process):
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline:
        sock = socket.socket(socket.AF_UNIX)
        try:
            sock.connect(str(path))
            sock.settimeout(5)
            return sock
        except (FileNotFoundError, ConnectionRefusedError):
            sock.close()
            if process.poll() is not None:
                raise RuntimeError('pump exited before listening')
            time.sleep(.01)
    raise TimeoutError(str(path))


class SerialPumpTest(unittest.TestCase):
    def test_reset_creates_boot_records(self):
        with tempfile.TemporaryDirectory(prefix='pump-', dir='/tmp') as tmp:
            run = Path(tmp)
            commit = 'a' * 40
            tools = 'b' * 40
            (run / 'build.json').write_text(json.dumps({'commit': commit, 'dirty': True}))
            (run / 'mode').write_text('console\n')
            (run / 'tools-commit').write_text(tools + '\n')
            monitor = socket.socket(socket.AF_UNIX)
            monitor.bind(str(run / 'events.sock'))
            monitor.listen()
            process = subprocess.Popen(['python3', str(PUMP), str(run), str(run / 'whole.txt')])
            try:
                guest = connect(run / 'serial.sock', process)
                monitor.settimeout(5)
                qmp, _ = monitor.accept()
                qmp.settimeout(5)
                qmp.sendall(b'{"QMP":{"version":{},"capabilities":[]}}\r\n')
                stream = qmp.makefile('rb')
                self.assertEqual(json.loads(stream.readline())['execute'], 'qmp_capabilities')
                qmp.sendall(b'{"return":{}}\r\n')
                first = (run / 'current').read_text().strip()
                qmp.sendall(b'{"event":"RESET","data":{"guest":true}}\r\n')
                deadline = time.monotonic() + 5
                while time.monotonic() < deadline and (run / 'current').read_text().strip() == first:
                    time.sleep(.01)
                second = (run / 'current').read_text().strip()
                self.assertNotEqual(first, second)
                qmp.sendall(b'{"event":"SHUTDOWN","data":{"guest":true}}\r\n')
                qmp.close()
                guest.shutdown(socket.SHUT_WR)
                self.assertEqual(process.wait(timeout=5), 0)
                boots = list((run / 'boots').iterdir())
                self.assertEqual(len(boots), 2)
                for directory in boots:
                    self.assertTrue((directory / 'ended').read_text().strip())
                    record = json.loads((directory / 'run.json').read_text())
                    self.assertEqual(record, {'mode': 'persistent', 'bootMode': 'console',
                                              'target': 'qemu', 'bootedCommit': commit,
                                              'toolsCommit': tools, 'evidenceEligible': False})
                    self.assertTrue((directory / 'events.jsonl').exists())
                self.assertEqual(json.loads((run / 'boots' / first / 'events.jsonl').read_text().splitlines()[0])['event'], 'RESET')
                self.assertEqual(json.loads((run / 'boots' / second / 'events.jsonl').read_text().splitlines()[0])['event'], 'SHUTDOWN')
                guest.close()
            finally:
                monitor.close()
                if process.poll() is None:
                    process.terminate()
                    process.wait(timeout=5)

    def test_capture_and_console_input(self):
        with tempfile.TemporaryDirectory(prefix='pump-', dir='/tmp') as tmp:
            run = Path(tmp)
            whole = run / 'whole.txt'
            process = subprocess.Popen(['python3', str(PUMP), str(run), str(whole)])
            try:
                guest = connect(run / 'serial.sock', process)
                console = connect(run / 'console.sock', process)
                output = b'first\x00\xff\r\n' + bytes(range(256)) * 100
                incoming = b'input\x00\xff\r\n' + bytes(range(256))
                guest.sendall(output)
                console.sendall(incoming)
                received = b''
                while len(received) < len(incoming):
                    received += guest.recv(len(incoming) - len(received))
                self.assertEqual(received, incoming)
                guest.shutdown(socket.SHUT_WR)
                self.assertEqual(process.wait(timeout=5), 0)
                boot = (run / 'current').read_text().strip()
                self.assertRegex(boot, r'^\d{8}T\d{6}Z-qemu$')
                directory = run / 'boots' / boot
                self.assertEqual((directory / 'serial.txt').read_bytes(), output)
                self.assertEqual(whole.read_bytes(), output)
                rows = [line.split('\t') for line in (directory / 'serial.ts').read_text().splitlines()]
                self.assertTrue(rows)
                stamps = [(float(t), int(offset)) for t, offset in rows]
                offsets = [offset for _, offset in stamps]
                self.assertEqual(offsets, sorted(offsets))
                self.assertEqual(offsets[-1], len(output))
                self.assertTrue(all(t > 0 for t, _ in stamps))
                guest.close()
                console.close()
            finally:
                if process.poll() is None:
                    process.terminate()
                    process.wait(timeout=5)


if __name__ == '__main__':
    unittest.main()
