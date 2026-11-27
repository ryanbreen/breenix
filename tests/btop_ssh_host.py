#!/usr/bin/env python3
"""Exercise the monitor, session relay and terminal hangup with deterministic I/O."""
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
FIXTURES = Path(__file__).with_name("fixtures") / "btop_ssh"


def source(path):
    return (ROOT / path).read_text()


def function(code, name):
    start = code.index(f"fn {name}(")
    body = code.index("{", start)
    depth = 1
    end = body + 1
    while depth:
        depth += (code[end] == "{") - (code[end] == "}")
        end += 1
    return code[start:end]


def check(name, **parts):
    code = (FIXTURES / f"{name}.rs").read_text()
    for field, value in parts.items():
        code = code.replace(f"// @{field}@", value)
    # Keep generated sources out of the checkout, including panic output paths.
    with tempfile.TemporaryDirectory() as scratch:
        path = Path(scratch)
        (path / "check.rs").write_text(code)
        subprocess.run(
            ["taskpolicy", "-b", "rustc", "--edition=2021", "check.rs", "-o", "check"],
            cwd=path, check=True, timeout=20,
        )
        subprocess.run(["./check"], cwd=path, check=True, timeout=5)


class BtopSsh(unittest.TestCase):
    def test_monitor_draws_at_pty_size_and_quits(self):
        monitor = source("userspace/programs/src/btop.rs")
        check("monitor", MONITOR="\n".join(
            line for line in monitor.splitlines() if not line.startswith("//!")
        ))

    def test_blocked_child_does_not_block_ssh_requests_or_disconnect(self):
        transport = source("libs/libbreenix/src/ssh/transport.rs")
        channel = source("libs/libbreenix/src/ssh/channel.rs")
        wire = source("libs/libbreenix/src/ssh/mod.rs")
        daemon = source("userspace/programs/src/bsshd.rs")
        check(
            "relay",
            CONSTANTS="\n".join(line for line in wire.splitlines() if "const SSH_MSG_" in line),
            BUFFER=wire[wire.index("pub struct SshBuf;"):],
            HANDLER=function(transport, "handle_channel_packet"),
            GLOBAL_REQUEST="pub(super) " + function(channel, "handle_client_global_request"),
            CHANNEL_REQUEST="pub(super) " + function(channel, "server_handle_channel_request"),
            SHUTTLE=function(daemon, "data_shuttle"),
            PTY_SIZE=function(daemon, "set_pty_size") if "fn set_pty_size(" in daemon else "",
        )

    def test_hangup_alerts_the_leader_and_foreground_job(self):
        ctty = source("kernel/src/tty/ctty.rs")
        check("hangup", HANGUP=function(ctty, "hangup_ref"))
        daemon = source("userspace/programs/src/bsshd.rs")
        shell = daemon[daemon.index("// Fork a child for the shell"):daemon.index("// Parent: shuttle")]
        self.assertIn("signal::SIGHUP", shell, "login must reset init's inherited SIGHUP ignore")
        self.assertIn("Sigaction::default_action()", shell)


if __name__ == "__main__":
    unittest.main()
