#!/usr/bin/env python3
"""Supervise a clamped Mac lane's QEMU in a fresh launchd task.

The private socket transfers the command, environment and console descriptors;
launchctl's job arguments contain only this helper and the socket. Closing the
supervisor's socket (including SIGKILL) stops QEMU before the job exits.
"""

import array
import json
import os
from pathlib import Path
import plistlib
import select
import shutil
import signal
import socket
import subprocess
import sys
import tempfile
import uuid


def interrupted(number, frame):
    raise SystemExit(128 + number)


def child(path):
    with socket.socket(socket.AF_UNIX) as channel:
        channel.connect(path)
        descriptors = array.array("i")
        data, ancillary, flags, _ = channel.recvmsg(1, socket.CMSG_SPACE(3 * descriptors.itemsize))
        if data != b"F" or flags & socket.MSG_CTRUNC:
            return 126
        for level, kind, contents in ancillary:
            if level == socket.SOL_SOCKET and kind == socket.SCM_RIGHTS:
                descriptors.frombytes(contents[:len(contents) - len(contents) % descriptors.itemsize])
        if len(descriptors) != 3:
            return 126
        with channel.makefile("rb") as source:
            request = json.loads(source.readline())
        env = request["env"]
        env["BREENIX_QEMU_UNCLAMPED"] = "1"
        try:
            process = subprocess.Popen(["/usr/sbin/taskpolicy", "-a", *request["argv"]], env=env, stdin=descriptors[0],
                                       stdout=descriptors[1], stderr=descriptors[2])
        except OSError as error:
            os.write(descriptors[2], f"qemu-unclamp: cannot launch boot: {error}\n".encode())
            raise
        finally:
            for fd in descriptors:
                os.close(fd)
        try:
            while process.poll() is None:
                if select.select([channel], [], [], 0.1)[0] and not channel.recv(1):
                    return 143
            channel.sendall(f"{process.returncode}\n".encode())
        finally:
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()
    return 0


def supervise(argv):
    label = "org.breenix.qemu." + uuid.uuid4().hex
    domain = f"gui/{os.getuid()}"
    # /tmp also keeps the socket below macOS's Unix socket path limit.
    with tempfile.TemporaryDirectory(prefix="breenix-qos-", dir="/tmp") as directory:
        path = str(Path(directory) / "launch.sock")
        job = Path(directory) / "boot.plist"
        job.write_bytes(plistlib.dumps({
            "Label": label, "ProcessType": "Interactive", "RunAtLoad": True,
            "ProgramArguments": [sys.executable, str(Path(__file__).resolve()), "--child", path, label],
        }))
        with socket.socket(socket.AF_UNIX) as listener:
            listener.bind(path)
            listener.listen(1)
            listener.settimeout(30)
            try:
                subprocess.run(["/bin/launchctl", "bootstrap", domain, str(job)], check=True)
                channel, _ = listener.accept()
                with channel:
                    descriptors = array.array("i", [0, 1, 2])
                    channel.sendmsg([b"F"], [(socket.SOL_SOCKET, socket.SCM_RIGHTS, descriptors)])
                    channel.sendall(json.dumps({"argv": argv, "env": dict(os.environ)}).encode() + b"\n")
                    with channel.makefile("rb") as source:
                        result = source.readline()
                    return int(result) if result else 126
            finally:
                subprocess.run(["/bin/launchctl", "bootout", f"{domain}/{label}"], stdout=subprocess.DEVNULL,
                               stderr=subprocess.DEVNULL)


if __name__ == "__main__":
    for number in (signal.SIGTERM, signal.SIGHUP, signal.SIGINT):
        signal.signal(number, interrupted)
    if sys.argv[1:2] == ["--child"]:
        try:
            status = child(sys.argv[2])
        finally:
            # The caller may have died by SIGKILL and cannot unload the job.
            # First reap QEMU (child's finally), then remove our private files
            # and service. launchd also kills any remaining job-group members.
            shutil.rmtree(Path(sys.argv[2]).parent, ignore_errors=True)
            subprocess.run(["/bin/launchctl", "bootout", f"gui/{os.getuid()}/{sys.argv[3]}"],
                           stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        sys.exit(status)
    status = supervise(sys.argv[1:])
    if status < 0:
        number = -status
        if number != signal.SIGKILL:
            signal.signal(number, signal.SIG_DFL)
        os.kill(os.getpid(), number)
    sys.exit(status)
