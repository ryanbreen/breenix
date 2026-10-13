"""Copy QEMU's Unix socket serial bytes into gate files without file-backend delay."""
from pathlib import Path
import socket
import tempfile
import threading
import warnings


class SerialRelay:
    def __init__(self, output):
        self.output = Path(output)
        self.socket = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.temporary = tempfile.TemporaryDirectory(prefix="breenix-serial-", dir="/tmp")
        self.path = str(Path(self.temporary.name) / "serial.sock")
        self.socket.bind(self.path)
        self.socket.listen(1)
        self.socket.settimeout(0.1)
        self.stopping = threading.Event()
        self.connection = None
        self.error = None
        self.thread = threading.Thread(target=self._copy, daemon=True)
        self.thread.start()

    def _copy(self):
        try:
            while not self.stopping.is_set():
                try:
                    connection, _ = self.socket.accept()
                    break
                except socket.timeout:
                    continue
            else:
                return
            self.connection = connection
            with connection, self.output.open("ab", buffering=0) as output:
                connection.settimeout(0.1)
                while True:
                    try:
                        data = connection.recv(65536)
                    except socket.timeout:
                        if self.stopping.is_set():
                            break
                        continue
                    if not data:
                        break
                    output.write(data)
        except OSError as error:
            if not self.stopping.is_set():
                self.error = error

    def close(self):
        self.stopping.set()
        self.thread.join(timeout=5)
        if self.thread.is_alive():
            if self.connection is not None:
                self.connection.close()
            self.thread.join(timeout=1)
        self.socket.close()
        self.temporary.cleanup()
        if self.thread.is_alive():
            raise RuntimeError(f"serial relay did not stop: {self.output}")
        if self.error is not None:
            raise RuntimeError(f"serial relay failed: {self.output}: {self.error}")


def live_serial_command(command, outputs):
    """Replace this gate's `-serial file:` arguments and return owned relays."""
    command = list(command)
    relays = []
    try:
        for output in outputs:
            target = "file:" + str(output)
            for index in range(1, len(command)):
                if command[index - 1] == "-serial" and command[index] == target:
                    relay = SerialRelay(output)
                    relays.append(relay)
                    command[index] = f"unix:{relay.path},server=off"
                    break
        if any(command[index - 1] == "-serial" and value.startswith("file:")
               for index, value in enumerate(command) if index > 0):
            raise ValueError("x86 gate still has a buffered file serial channel")
        return command, relays
    except BaseException:
        close_relays(relays)
        raise


def close_relays(relays):
    for relay in relays:
        try:
            relay.close()
        except (OSError, RuntimeError) as error:
            warnings.warn(str(error), RuntimeWarning)
