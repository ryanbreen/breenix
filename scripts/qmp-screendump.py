#!/usr/bin/env python3
"""Save the VM screen as a PNG through QEMU's QMP socket.

    scripts/qmp-screendump.py SOCKET OUT.png

SOCKET is the path given to `scripts/boot-interactive.sh --qmp SOCKET`. Works with
`-display none`: QEMU keeps the guest console's surface and dumps it on request.
"""

import json
import os
import socket
import sys


def main():
    if len(sys.argv) != 3:
        print(__doc__.strip(), file=sys.stderr)
        return 2
    sock_path, out = sys.argv[1], os.path.abspath(sys.argv[2])
    sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    sock.settimeout(10)
    sock.connect(sock_path)
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
    msg = command("screendump", {"filename": out, "format": "png"})
    if "error" in msg:
        print("screendump failed: %s" % msg["error"].get("desc", msg), file=sys.stderr)
        return 1
    print(out)
    return 0


if __name__ == "__main__":
    sys.exit(main())
