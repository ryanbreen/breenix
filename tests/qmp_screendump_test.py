"""Run scripts/qmp-screendump.py against a stub QMP server and decode the PNG it writes."""

import json
import os
from pathlib import Path
import socket
import struct
import subprocess
import sys
import tempfile
import threading
import time
import unittest
import zlib


ROOT = Path(__file__).resolve().parents[1]
WIDTH, HEIGHT = 5, 3


def pixel(x, y):
    return bytes(((x * 40) % 256, (y * 90) % 256, (x * y * 17) % 256))


class StubQmp(threading.Thread):
    """Answers one connection: greeting, capabilities, then a screendump it writes as QEMU's ppm_save does."""

    def __init__(self, path, error=None, listen_after=0):
        super().__init__(daemon=True)
        self.server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.server.bind(path)
        self.listen_after = listen_after
        if not listen_after:
            self.server.listen(1)
        self.error = error
        self.requests = []

    def run(self):
        if self.listen_after:
            # A bound socket that does not listen refuses connections, as a busy QMP socket does.
            time.sleep(self.listen_after)
            self.server.listen(1)
        conn, _ = self.server.accept()
        with conn, conn.makefile("rw") as stream:
            stream.write(json.dumps({"QMP": {"version": {}, "capabilities": []}}) + "\n")
            stream.flush()
            for line in stream:
                req = json.loads(line)
                self.requests.append(req)
                if req["execute"] == "screendump" and self.error:
                    stream.write(json.dumps({"error": {"class": "GenericError", "desc": self.error}}) + "\n")
                elif req["execute"] == "screendump":
                    rows = b"".join(pixel(x, y) for y in range(HEIGHT) for x in range(WIDTH))
                    Path(req["arguments"]["filename"]).write_bytes(b"P6\n%d %d\n255\n" % (WIDTH, HEIGHT) + rows)
                    stream.write(json.dumps({"timestamp": {}, "event": "RESUME"}) + "\n")
                    stream.write(json.dumps({"return": {}}) + "\n")
                else:
                    stream.write(json.dumps({"return": {}}) + "\n")
                stream.flush()
        self.server.close()


def decode_png(data):
    assert data[:8] == b"\x89PNG\r\n\x1a\n"
    at, chunks = 8, {}
    while at < len(data):
        (length,) = struct.unpack(">I", data[at:at + 4])
        kind, body = data[at + 4:at + 8], data[at + 8:at + 8 + length]
        (crc,) = struct.unpack(">I", data[at + 8 + length:at + 12 + length])
        assert crc == zlib.crc32(kind + body), kind
        chunks.setdefault(kind, b"")
        chunks[kind] += body
        at += 12 + length
    width, height, depth, colour, _, _, _ = struct.unpack(">IIBBBBB", chunks[b"IHDR"])
    assert (depth, colour) == (8, 2) and b"IEND" in chunks
    raw = zlib.decompress(chunks[b"IDAT"])
    stride = width * 3 + 1
    rows = [raw[y * stride:(y + 1) * stride] for y in range(height)]
    assert all(row[0] == 0 for row in rows)
    return width, height, b"".join(row[1:] for row in rows)


class QmpScreendumpTest(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory(prefix="qs-", dir="/tmp")
        self.addCleanup(tmp.cleanup)
        self.dir = Path(tmp.name)
        self.sock = str(self.dir / "qmp.sock")
        self.out = self.dir / "shots" / "out.png"
        self.out.parent.mkdir()

    def dump(self, stub):
        stub.start()
        result = subprocess.run([sys.executable, str(ROOT / "scripts/qmp-screendump.py"), self.sock, str(self.out)],
                                capture_output=True, text=True, timeout=30)
        stub.join(10)
        return result

    def test_qemu_writes_ppm_and_the_tool_writes_the_png(self):
        stub = StubQmp(self.sock)
        result = self.dump(stub)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.strip(), str(self.out))
        dumps = [r for r in stub.requests if r["execute"] == "screendump"]
        self.assertEqual(len(dumps), 1)
        self.assertEqual(dumps[0]["arguments"]["format"], "ppm")
        self.assertEqual(Path(dumps[0]["arguments"]["filename"]).parent, self.out.parent)
        width, height, pixels = decode_png(self.out.read_bytes())
        self.assertEqual((width, height), (WIDTH, HEIGHT))
        self.assertEqual(pixels, b"".join(pixel(x, y) for y in range(HEIGHT) for x in range(WIDTH)))
        self.assertEqual(os.listdir(self.out.parent), ["out.png"])

    def test_a_refused_connection_is_retried(self):
        stub = StubQmp(self.sock, listen_after=0.5)
        result = self.dump(stub)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(decode_png(self.out.read_bytes())[:2], (WIDTH, HEIGHT))

    def test_a_refused_screendump_fails_and_leaves_nothing(self):
        result = self.dump(StubQmp(self.sock, error="no surface"))
        self.assertEqual(result.returncode, 1)
        self.assertIn("screendump failed: no surface", result.stderr)
        self.assertEqual(os.listdir(self.out.parent), [])


if __name__ == "__main__":
    unittest.main()
