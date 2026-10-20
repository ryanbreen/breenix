#!/usr/bin/env python3
"""Save the VM screen as a PNG through QEMU's QMP socket.

    scripts/qmp-screendump.py SOCKET OUT.png

SOCKET is the path given to `scripts/boot-interactive.sh --qmp SOCKET`. Works with
`-display none`: QEMU keeps the guest console's surface and dumps it on request.

QEMU writes a screendump on its main loop while holding the lock every vCPU takes to
leave the guest, which an ARM64 HVF guest does for each timer interrupt. A PNG dump of
the 1280x800 console holds it about 17 ms, as long as it compresses; a PPM dump holds it
about 3 ms. So QEMU writes PPM beside OUT and this process encodes the PNG.
"""

import json
import os
import socket
import struct
import sys
import tempfile
import time
import zlib


def read_ppm(path):
    """Width, height and RGB rows of a binary (P6) PPM with 8-bit samples."""
    with open(path, "rb") as f:
        data = f.read()
    fields = []
    at = 0
    while len(fields) < 4:
        while data[at:at + 1].isspace():
            at += 1
        end = at
        while end < len(data) and not data[end:end + 1].isspace():
            end += 1
        fields.append(data[at:end])
        at = end
    magic, width, height, maxval = fields[0], int(fields[1]), int(fields[2]), int(fields[3])
    if magic != b"P6" or maxval != 255:
        raise ValueError("not an 8-bit P6 PPM")
    pixels = data[at + 1:]
    # QEMU's ppm_save writes each row from a pixman line buffer, padded to 4 bytes.
    row = width * 3
    stride = (row + 3) & ~3
    if len(pixels) < stride * height:
        raise ValueError("PPM is shorter than its %dx%d header" % (width, height))
    return width, height, [pixels[y * stride:y * stride + row] for y in range(height)]


def write_png(path, width, height, rows):
    def chunk(kind, body):
        return struct.pack(">I", len(body)) + kind + body + struct.pack(">I", zlib.crc32(kind + body))

    image = zlib.compress(b"".join(b"\0" + row for row in rows), 6)
    with open(path, "wb") as f:
        f.write(b"\x89PNG\r\n\x1a\n")
        f.write(chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)))
        f.write(chunk(b"IDAT", image))
        f.write(chunk(b"IEND", b""))


def main():
    if len(sys.argv) != 3:
        print(__doc__.strip(), file=sys.stderr)
        return 2
    sock_path, out = sys.argv[1], os.path.abspath(sys.argv[2])
    # QEMU serves one QMP client at a time and queues one more; macOS refuses a connection
    # beyond that, as when Vigil's capture and another overlap, so a refusal is retried.
    deadline = time.monotonic() + 5
    while True:
        sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        sock.settimeout(10)
        try:
            sock.connect(sock_path)
            break
        except ConnectionRefusedError:
            sock.close()
            if time.monotonic() > deadline:
                raise
            time.sleep(0.05)
    stream = sock.makefile("rw")

    def reply():
        # Skip asynchronous events until the command's return or error arrives.
        while True:
            line = stream.readline()
            if not line:
                raise RuntimeError("QMP socket closed")
            msg = json.loads(line)
            if "return" in msg or "error" in msg:
                return msg

    def command(name, arguments=None):
        req = {"execute": name}
        if arguments:
            req["arguments"] = arguments
        stream.write(json.dumps(req) + "\n")
        stream.flush()
        return reply()

    json.loads(stream.readline())  # greeting
    command("qmp_capabilities")
    fd, ppm = tempfile.mkstemp(prefix=".screendump-", suffix=".ppm", dir=os.path.dirname(out))
    os.close(fd)
    try:
        msg = command("screendump", {"filename": ppm, "format": "ppm"})
        if "error" in msg:
            print("screendump failed: %s" % msg["error"].get("desc", msg), file=sys.stderr)
            return 1
        write_png(out, *read_ppm(ppm))
    finally:
        os.unlink(ppm)
    print(out)
    return 0


if __name__ == "__main__":
    sys.exit(main())
