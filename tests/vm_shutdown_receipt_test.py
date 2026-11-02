#!/usr/bin/env python3
"""Exercise serial receipts and QMP transitions without starting a VM."""
import hashlib
import json
import socket
import subprocess
import tempfile
import time
import unittest
from pathlib import Path

from vm_serial_pump_test import PUMP, connect


class ReceiptTest(unittest.TestCase):
    def exercise(self, nonce=True, valid=True, reason='guest-reset', action='reboot'):
        with tempfile.TemporaryDirectory(prefix='receipt-', dir='/tmp') as tmp:
            run = Path(tmp)
            token, challenge = 'a' * 64, 'b' * 64
            (run / 'power-token').write_text(token + '\n')
            monitor = socket.socket(socket.AF_UNIX)
            monitor.bind(str(run / 'events.sock'))
            monitor.listen()
            monitor.settimeout(5)
            process = subprocess.Popen(['python3', str(PUMP), str(run), str(run / 'whole')])
            guest = qmp = None
            try:
                guest = connect(run / 'serial.sock', process)
                qmp, _ = monitor.accept()
                qmp.sendall(b'{"QMP":{}}\r\n')
                self.assertIn(b'qmp_capabilities', qmp.recv(1024))
                first = (run / 'current').read_text().strip()
                directory = run / 'boots' / first
                digest = hashlib.sha256((token + challenge).encode()).hexdigest() if valid else '0' * 64
                data = (('[init] shutdown nonce=' + challenge + '\r\n') if nonce else '') + f'[shutdown] requested {action} token={digest}\r\n'
                for piece in (data[:17], data[17:]):
                    guest.sendall(piece.encode())
                deadline = time.monotonic() + 5
                while data.replace('\r\n', '\n') not in (directory / 'serial.txt').read_text():
                    if time.monotonic() > deadline:
                        self.fail('serial not captured')
                    time.sleep(.01)
                event = 'SHUTDOWN' if 'shutdown' in reason or reason == 'host-quit' else 'RESET'
                qmp.sendall((json.dumps({'event': event, 'data': {'reason': reason, 'guest': reason.startswith('guest-')}}) + '\n').encode())
                if event == 'RESET':
                    deadline = time.monotonic() + 5
                    while (run / 'current').read_text().strip() == first:
                        if time.monotonic() > deadline:
                            self.fail('reset not captured')
                        time.sleep(.01)
                    successor = (run / 'current').read_text().strip()
                else:
                    deadline = time.monotonic() + 5
                    while 'SHUTDOWN' not in (directory / 'events.jsonl').read_text():
                        if time.monotonic() > deadline:
                            self.fail('shutdown not captured')
                        time.sleep(.01)
                    successor = None
                guest.shutdown(socket.SHUT_WR)
                self.assertEqual(process.wait(timeout=5), 0)
                events = [json.loads(line) for line in (directory / 'events.jsonl').read_text().splitlines()]
                requests = [e for e in events if e['event'] == 'requested-shutdown']
                resets = [e for e in events if e['event'] == 'requested-reset-observed']
                self.assertEqual([e['action'] for e in requests], [action] if nonce and valid else [])
                if nonce and valid and action == 'reboot' and reason == 'guest-reset':
                    self.assertEqual(len(resets), 1)
                    self.assertEqual(resets[0]['successor'], successor)
                else:
                    self.assertEqual(resets, [])
                status = directory / 'exit-status'
                self.assertEqual(status.read_text().strip() if status.exists() else None,
                                 '0' if reason == 'guest-shutdown' else None)
            finally:
                if guest is not None:
                    guest.close()
                if qmp is not None:
                    qmp.close()
                monitor.close()
                if process.poll() is None:
                    process.terminate()
                    process.wait(timeout=5)

    def test_guest_reset_with_matching_receipt(self):
        self.exercise()

    def test_wrong_token_and_missing_nonce(self):
        self.exercise(valid=False)
        self.exercise(nonce=False)

    def test_host_reset_and_quit(self):
        self.exercise(reason='host-qmp-system-reset')
        self.exercise(reason='host-quit', action='poweroff')

    def test_guest_shutdown(self):
        self.exercise(reason='guest-shutdown', action='poweroff')


if __name__ == '__main__':
    unittest.main()
