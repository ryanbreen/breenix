#!/usr/bin/env python3
import json
import re
import runpy
import socket
import subprocess
import tempfile
import threading
import time
import unittest
from pathlib import Path

PUMP = Path(__file__).resolve().parents[1] / 'scripts/vm-serial-pump.py'
CLEANER = runpy.run_path(str(PUMP.parent / 'metal/metal_lib.py'))['SerialCleaner']
HOST = runpy.run_path(str(PUMP.parent / 'metal/server-suite'))


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
    def test_retained_shell_echo_and_kill_through_target(self):
        # Shell-only bytes from the failed 20261101T210030Z capture at 214e7a3b.
        # No login, site values or kernel records are retained in the fixture.
        output = (Path(__file__).parent / 'fixtures/console-input-redraw.raw').read_bytes()
        clear = b'\rbsh root> \x1b[K'
        echo, exit_echo, tail = output.split(clear)
        self.assertEqual(tail, b'')
        with tempfile.TemporaryDirectory(prefix='pump-', dir='/tmp') as tmp:
            run = Path(tmp)
            process = subprocess.Popen(['python3', str(PUMP), str(run), str(run / 'whole.txt')])
            try:
                guest = connect(run / 'serial.sock', process)
                self.addCleanup(guest.close)
                target = object.__new__(HOST['QemuTarget'])
                target.run = run
                target.boot = (run / 'current').read_text().strip()
                target.directory = run / 'boots' / target.boot
                target.holds_lease = lambda: True  # Socket peer replaces QEMU in this host test.
                console = target.console('', '')
                original_send = console.send
                sent = []
                def send(text, enter=True):
                    original_send(text, enter)
                    expected = (text + ('\r' if enter else '')).encode()
                    received = b''
                    while len(received) < len(expected):
                        received += guest.recv(len(expected) - len(received))
                    self.assertEqual(received, expected)
                    sent.append(received)
                    response = {b'print("xxxxxxxxx': echo, b'\x15': clear, b'exit': exit_echo}.get(received, b'')
                    # Split a CR and an ANSI sequence across socket writes too.
                    for at in range(0, len(response), 7):
                        guest.sendall(response[at:at + 7])
                console.send = send
                original_wait = console.wait
                console.wait = lambda offset, pattern, what, **kw: original_wait(offset, pattern, what, timeout=1)
                console.type_line('print("xxxxxxxxx', attempts=1)
                offset = console.size()
                console.send('\x15', enter=False)
                console.wait(offset, r'(?m)^' + console.PROMPT + r' *(?=\[init\]|\n|$)', 'the cleared console line')
                console.type_line('exit', attempts=1)
                offset = console.size()
                console.send('\x15', enter=False)
                console.wait(offset, r'(?m)^' + console.PROMPT + r' *(?=\[init\]|\n|$)', 'the cleared console line')
                self.assertEqual(sent, [b'print("xxxxxxxxx', b'\r', b'\x15', b'exit', b'\r', b'\x15'])
                guest.shutdown(socket.SHUT_WR)
                self.assertEqual(process.wait(timeout=5), 0)
                self.assertEqual((target.directory / 'serial.raw').read_bytes(), output)
            finally:
                if process.poll() is None:
                    process.terminate()
                    process.wait(timeout=5)

    def test_short_lived_console_connections_keep_pump_alive(self):
        # QemuTarget opens one connection per four input bytes. Closing with
        # unread echo can leave separate read/write events in a kqueue batch.
        with tempfile.TemporaryDirectory(prefix='pump-', dir='/tmp') as tmp:
            run = Path(tmp)
            # Deliver EOF and an already-ready writable notification separately,
            # as kqueue can. Keep the actual sockets, pump and target transport.
            bootstrap = r'''import runpy, selectors, socket, sys
base = selectors.DefaultSelector
class SplitReady(base):
    def select(self, timeout=None):
        result = []
        for key, mask in super().select(timeout):
            if key.data == 'console-data' and mask & selectors.EVENT_READ:
                try:
                    eof = key.fileobj.recv(1, socket.MSG_PEEK) == b''
                except (BlockingIOError, ConnectionResetError):
                    eof = False
                if eof:
                    result.extend([(key, selectors.EVENT_READ), (key, selectors.EVENT_WRITE)])
                    continue
            result.append((key, mask))
        return result
selectors.DefaultSelector = SplitReady
sys.argv = sys.argv[1:]
runpy.run_path(sys.argv[0], run_name='__main__')
'''
            with (run / 'stderr').open('w+') as errors:
                process = subprocess.Popen(['python3', '-c', bootstrap, str(PUMP), str(run), str(run / 'whole.txt')], stderr=errors)
                guest = connect(run / 'serial.sock', process)
                received = bytearray()
                def echo():
                    try:
                        while data := guest.recv(65536):
                            received.extend(data)
                            guest.sendall(b'echo ' + data + b'x' * 1024 + b'\n')
                    except OSError:
                        pass
                worker = threading.Thread(target=echo)
                worker.start()
                try:
                    for _ in range(256):
                        console = connect(run / 'console.sock', process)
                        console.sendall(b'abcd')
                        time.sleep(.002)
                        console.close()
                    deadline = time.monotonic() + 5
                    while len(received) < 1024 and time.monotonic() < deadline:
                        time.sleep(.01)
                    self.assertEqual(bytes(received), b'abcd' * 256)
                    self.assertIsNone(process.poll(), 'pump exited after a console disconnect')
                    guest.shutdown(socket.SHUT_WR)
                    self.assertEqual(process.wait(timeout=5), 0)
                finally:
                    guest.close()
                    worker.join(timeout=5)
                    if process.poll() is None:
                        process.terminate()
                        process.wait(timeout=5)
                    errors.seek(0)
                    failure = errors.read()
                    self.assertEqual(failure, '', failure)

    def test_reset_creates_boot_records(self):
        with tempfile.TemporaryDirectory(prefix='pump-', dir='/tmp') as tmp:
            run = Path(tmp)
            commit = subprocess.check_output(['git', '-C', str(PUMP.parent.parent), 'rev-parse', 'HEAD'], text=True).strip()
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
                self.addCleanup(guest.close)
                monitor.settimeout(5)
                qmp, _ = monitor.accept()
                qmp.settimeout(5)
                qmp.sendall(b'{"QMP":{"version":{},"capabilities":[]}}\r\n')
                stream = qmp.makefile('rb')
                self.addCleanup(stream.close)
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
                    self.assertEqual(record, {'id': directory.name, 'mode': 'persistent', 'bootMode': 'console',
                                              'target': 'qemu', 'bootedCommit': commit,
                                              'toolsCommit': tools, 'evidenceEligible': False})
                    self.assertTrue((directory / 'events.jsonl').exists())
                    for name in ('docs/suites/server.json', 'docs/suites/users.json',
                                 'docs/efforts/path.json', 'docs/boot-path.json'):
                        expected = subprocess.check_output(['git', '-C', str(PUMP.parent.parent),
                                                            'show', commit + ':' + name])
                        self.assertEqual((directory / 'definition' / name).read_bytes(), expected)
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
                self.addCleanup(guest.close)
                console = connect(run / 'console.sock', process)
                self.addCleanup(console.close)
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
                # The target boundary consumes the boot-store contract in docs/metal/README.md.
                self.assertEqual((directory / 'serial.raw').read_bytes(), output)
                cleaned = CLEANER().feed(output).encode()
                self.assertEqual((directory / 'serial.txt').read_bytes(), cleaned)
                self.assertEqual(whole.read_bytes(), output)
                rows = [line.split('\t') for line in (directory / 'serial.ts').read_text().splitlines()]
                self.assertTrue(rows)
                stamps = [(float(t), int(offset)) for t, offset in rows]
                offsets = [offset for _, offset in stamps]
                self.assertEqual(offsets, [0] + [i + 1 for i, byte in enumerate(cleaned)
                                                  if byte == 10 and i + 1 < len(cleaned)])
                self.assertTrue(all(t > 0 for t, _ in stamps))
                guest.close()
                console.close()
            finally:
                if process.poll() is None:
                    process.terminate()
                    process.wait(timeout=5)

    def test_fragmented_crash_loop_has_one_line_start(self):
        with tempfile.TemporaryDirectory(prefix='pump-', dir='/tmp') as tmp:
            run = Path(tmp)
            process = subprocess.Popen(['python3', str(PUMP), str(run), str(run / 'whole.txt')])
            try:
                guest = connect(run / 'serial.sock', process)
                self.addCleanup(guest.close)
                boot = (run / 'current').read_text().strip()
                directory = run / 'boots' / boot
                prefix = b'previous line\r\n'
                fragments = [prefix + b'[init] svc-suite-fail is crash-',
                             b'looping: 5 failures in a row', b', each within 10 s of starting\r\n']
                sent = b''
                for fragment in fragments:
                    guest.sendall(fragment)
                    sent += fragment
                    deadline = time.monotonic() + 5
                    while (directory / 'serial.raw').stat().st_size < len(sent):
                        if time.monotonic() >= deadline:
                            self.fail('pump did not capture a fragment')
                        time.sleep(.01)
                guest.shutdown(socket.SHUT_WR)
                self.assertEqual(process.wait(timeout=5), 0)
                target = object.__new__(HOST['QemuTarget'])
                target.directory = directory
                target.holds_lease = lambda: True
                text, stamps = target.capture()
                self.assertEqual([offset for _, offset in stamps], [0, len(b'previous line\n')])
                services = HOST['Services'](None, None, target.capture)
                matches = services.lines(len(b'previous line\n'),
                                         r'\[init\] svc-suite-fail is crash-looping: 5 failures in a row')
                self.assertEqual(matches, [(stamps[1][0], text.splitlines()[1])])
            finally:
                if process.poll() is None:
                    process.terminate()
                    process.wait(timeout=5)


if __name__ == '__main__':
    unittest.main()
