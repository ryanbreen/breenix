#!/usr/bin/env python3
"""Drive two non-PTY execs against a Breenix bsshd using OpenSSH."""

import subprocess
import sys
import tempfile
from pathlib import Path


def check(destination, ssh_options):
    with tempfile.TemporaryDirectory(prefix="bsshd-") as directory:
        control = str(Path(directory) / "c")
        log = Path(directory) / "wire.log"
        ssh = [*ssh_options, "-o", "BatchMode=yes", "-S", control]

        def run(*args, mux_only=False):
            # If the control socket is gone, fail before trying a new transport.
            mux = ["-o", "ControlMaster=no", "-o", "ProxyCommand=false"] if mux_only else []
            return subprocess.run(["ssh", *mux, *ssh, *args], capture_output=True, timeout=15)

        try:
            master = run("-vvv", "-E", str(log), "-fN", "-o", "ControlMaster=yes", destination)
            if master.returncode:
                raise RuntimeError(f"master failed: {master.stderr.decode(errors='replace')}")

            offset = 0
            for index in (1, 2):
                marker = f"ssh-channel-{index}"
                result = run("-T", destination, "/bin/echo", marker, mux_only=True)
                wire = log.read_text(errors="replace")
                if result.returncode != 0 or result.stdout != f"{marker}\n".encode():
                    raise RuntimeError(
                        f"channel {index}: status={result.returncode}, "
                        f"stdout={result.stdout!r}, stderr={result.stderr!r}"
                    )
                trace = wire[offset:]
                confirmation = trace.find("receive packet: type 91")
                status = trace.find("rtype exit-status")
                peer_close = trace.find("receive packet: type 97")
                local_close = trace.find("send packet: type 97")
                if not 0 <= confirmation < status < min(peer_close, local_close):
                    raise RuntimeError(f"channel {index}: missing confirmation, status or close")
                offset = len(wire)
                print(f"channel {index}: confirmed, {marker}, exit 0, closed in both directions")
        except (RuntimeError, subprocess.TimeoutExpired):
            if log.exists():
                print(log.read_text(errors="replace"), file=sys.stderr)
            raise
        finally:
            if Path(control).exists():
                run("-O", "exit", destination)


if __name__ == "__main__":
    if len(sys.argv) < 2:
        sys.exit(f"Usage: {sys.argv[0]} DESTINATION [SSH OPTION ...]")
    check(sys.argv[1], sys.argv[2:])
