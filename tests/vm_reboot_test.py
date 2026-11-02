#!/usr/bin/env python3
"""Exercise in-run reboot through the pump without starting a VM."""
import os
import socket
import subprocess
import tempfile
import threading
import time
import unittest
from pathlib import Path

from vm_serial_pump_test import PUMP, connect

ROOT = PUMP.parent.parent
LAUNCHER = ROOT / 'scripts/vm-persistent.sh'


class RebootTest(unittest.TestCase):
    def exercise(self, reset):
        with tempfile.TemporaryDirectory(prefix='reboot-', dir='/tmp') as tmp:
            root = Path(tmp)
            run = root / 'run'
            run.mkdir()
            (run / '.breenix-vm-persistent').touch()
            (run / 'boot.lock').mkdir()
            site = root / '.config/breenix/vm-site'
            site.parent.mkdir(parents=True)
            fields = ('OPERATOR_USER OPERATOR_UID OPERATOR_PASSWORD OPERATOR_PASSWORD_HASH '
                      'OPERATOR_SSH_PUBLIC_KEY CONSOLE_USER CONSOLE_PASSWORD SNTP_SERVERS ROOT_PASSWORD_HASH')
            site.write_text(''.join(f'{key}={"root" if key == "CONSOLE_USER" else "secret"}\n' for key in fields.split()))
            site.chmod(0o600)
            events = socket.socket(socket.AF_UNIX)
            events.bind(str(run / 'events.sock'))
            events.listen()
            qmp = socket.socket(socket.AF_UNIX)
            qmp.bind(str(run / 'qmp.sock'))
            qmp.listen()
            qmp.settimeout(.1)
            stop = threading.Event()
            def serve_qmp():
                while not stop.is_set():
                    try:
                        client, _ = qmp.accept()
                    except socket.timeout:
                        continue
                    with client:
                        client.sendall(b'{"QMP":{"version":{},"capabilities":[]}}\r\n')
            server = threading.Thread(target=serve_qmp)
            server.start()
            pump = subprocess.Popen(['python3', str(PUMP), str(run), str(run / 'whole.txt')])
            proc = None
            try:
                guest = connect(run / 'serial.sock', pump)
                guest.settimeout(10)
                events.settimeout(5)
                monitor, _ = events.accept()
                monitor.sendall(b'{"QMP":{"version":{},"capabilities":[]}}\r\n')
                self.assertIn(b'qmp_capabilities', monitor.recv(1024))
                monitor.sendall(b'{"return":{}}\r\n')
                old = (run / 'current').read_text().strip()
                env = dict(os.environ, HOME=str(root), BREENIX_VM_REBOOT_TIMEOUT='2')
                proc = subprocess.Popen([str(LAUNCHER), 'qemu', 'reboot', '--run-dir', str(run)],
                                        stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=env)
                received = bytearray()
                def receive_line():
                    line = bytearray()
                    while not line.endswith(b'\r'):
                        data = guest.recv(1)
                        self.assertTrue(data)
                        line.extend(data)
                        received.extend(data)
                    return bytes(line)
                self.assertEqual(receive_line(), b'\r')
                guest.sendall(b'Breenix login: ')
                self.assertEqual(receive_line(), b'root\r')
                guest.sendall(b'Password: ')
                self.assertEqual(receive_line(), b'secret\r')
                guest.sendall(b'bsh / > ')
                # Console.type_line waits for each complete echo block.
                line = bytearray()
                while True:
                    byte = guest.recv(1)
                    self.assertTrue(byte)
                    received.extend(byte)
                    if byte == b'\r':
                        break
                    line.extend(byte)
                    guest.sendall(b'\nbsh / > ' + line)
                self.assertEqual(bytes(line), b'/sbin/reboot')
                if reset:
                    monitor.sendall(b'{"event":"RESET","data":{"guest":true}}\r\n')
                    deadline = time.monotonic() + 5
                    while time.monotonic() < deadline and (run / 'current').read_text().strip() == old:
                        time.sleep(.01)
                    new = (run / 'current').read_text().strip()
                    self.assertNotEqual(old, new)
                    self.assertTrue((run / 'boots' / old / 'ended').exists())
                    guest.sendall(b'Breenix login: ')
                out, err = proc.communicate(timeout=10)
                if reset:
                    self.assertEqual(proc.returncode, 0, err.decode())
                    self.assertIn(old.encode(), out)
                    self.assertIn(new.encode(), out)
                else:
                    self.assertNotEqual(proc.returncode, 0)
                    self.assertIn(b'timed out', err.lower())
                    self.assertEqual((run / 'current').read_text().strip(), old)
                self.assertEqual(bytes(received), b'\rroot\rsecret\r/sbin/reboot\r')
                monitor.close()
                guest.close()
            finally:
                if proc is not None and proc.poll() is None:
                    proc.terminate()
                    proc.wait(timeout=5)
                if pump.poll() is None:
                    pump.terminate()
                    pump.wait(timeout=5)
                stop.set()
                server.join(timeout=5)
                qmp.close()
                events.close()

    def test_reboot_waits_for_reset_and_login(self):
        self.exercise(True)

    def test_reboot_times_out_without_reset(self):
        self.exercise(False)


if __name__ == '__main__':
    unittest.main()
