"""The x86 gate must see a short serial line before QEMU exits."""
import runpy
import shutil
import subprocess
from pathlib import Path
import socket
import tempfile
import time
import unittest

ROOT = Path(__file__).resolve().parents[1]
relay_api = runpy.run_path(str(ROOT / "scripts/x86_serial_relay.py"))


class SerialRelayTest(unittest.TestCase):
    def test_qemu_accepts_client_serial_backend(self):
        qemu = shutil.which("qemu-system-x86_64")
        if qemu is None:
            self.skipTest("QEMU is not installed")
        with tempfile.TemporaryDirectory() as directory:
            serial = Path(directory) / "serial_user.log"
            command, relays = relay_api["live_serial_command"](
                [qemu, "-machine", "none", "-nodefaults", "-display", "none",
                 "-S", "-serial", "file:" + str(serial)], (serial,))
            process = None
            try:
                process = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
                deadline = time.monotonic() + 3
                while relays[0].connection is None and process.poll() is None and time.monotonic() < deadline:
                    time.sleep(.01)
                if process.poll() is not None:
                    self.fail(process.communicate()[1].decode(errors="replace"))
                self.assertIsNotNone(relays[0].connection, "QEMU did not connect to the serial relay")
            finally:
                if process is not None:
                    process.terminate()
                    process.communicate(timeout=5)
                relay_api["close_relays"](relays)

    def test_short_userspace_line_arrives_while_connection_is_open(self):
        with tempfile.TemporaryDirectory() as directory:
            user = Path(directory) / "serial_user.log"
            kernel = Path(directory) / "serial_kernel.log"
            command, relays = relay_api["live_serial_command"](
                ["qemu-uefi", "-serial", "file:" + str(user),
                 "-serial", "file:" + str(kernel)], (user, kernel))
            try:
                self.assertEqual(len(relays), 2)
                self.assertTrue(command[2].startswith("unix:"))
                self.assertTrue(command[4].startswith("unix:"))
                with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as stream:
                    stream.connect(relays[0].path)
                    stream.sendall(b"SIGKILL_TEARDOWN_TEST: FAIL\n")
                    deadline = time.monotonic() + 1
                    while (not user.exists() or not user.read_bytes()) and time.monotonic() < deadline:
                        time.sleep(.01)
                    self.assertEqual(user.read_bytes(), b"SIGKILL_TEARDOWN_TEST: FAIL\n")
            finally:
                relay_api["close_relays"](relays)


if __name__ == "__main__":
    unittest.main()
