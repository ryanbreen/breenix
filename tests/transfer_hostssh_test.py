"""Exercise the host SSH adapter without opening a network connection."""

import json
import os
import subprocess
import unittest
import tempfile
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import Mock, patch

from metal_tools_test import load_script


host = load_script("transfer-hostssh")


class TransferHostSsh(unittest.TestCase):
    def test_qemu_toolchain_stage_writes_settings_only(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            args = SimpleNamespace(run_dir=root, image=root / "disk", sha="a" * 40,
                                   toolchain=True, artifact=None, binary=None)
            def keygen(command, **kwargs):
                (root / "identity.pub").write_text("ssh-ed25519 fixture-key comment\n")
            with patch.object(host, "run", side_effect=keygen), \
                 patch.object(host, "require_e2fsprogs", return_value=("debugfs",)), \
                 patch.object(host.transfer, "stage") as stage:
                host.stage_qemu(args)
            self.assertEqual(stage.call_count, 1)
            self.assertEqual(stage.call_args.args, (args.image, "debugfs"))
            self.assertEqual(stage.call_args.kwargs, {"settings":
                "user breenix-transfer\nkey ssh-ed25519 fixture-key\nuid 1000\npeer 10.0.2.2\n"})
            self.assertFalse((root / host.transfer.ARTIFACT).exists())

    def test_qemu_toolchain_watch_answers_hello_over_ssh(self):
        nonce, digest = "a" * 32, "c" * 64
        fingerprint = "SHA256:" + "B" * 43
        user = "breenix-transfer"
        path = f"/home/{user}/.toolchain-{nonce}/hello"
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "client.json").write_text(json.dumps({"user": user, "port": 12345}))
            serial = root / "serial.txt"
            serial.write_text(f"SUITE toolchain HOSTSSH c-core/hello-over-ssh nonce={nonce} "
                              f"fp={fingerprint} peer=10.0.2.2 user={user} uid=1000 gid=1001\n")
            args = SimpleNamespace(run_dir=root, serial=serial, parent=os.getpid(), wait=1, toolchain=True)
            client = host.Client("127.0.0.1", 12345, user, "10.0.2.2", root / "identity", lambda: True)
            client.pin = Mock(return_value=True)
            client.ssh = Mock(side_effect=[
                subprocess.CompletedProcess([], 0, (nonce + "\n").encode(), b""),
                subprocess.CompletedProcess([], 0, f"{nonce}\n1000\n{path}\n{digest}\nhello\n".encode(), b""),
                subprocess.CompletedProcess([], 0, b"", b""),
                subprocess.CompletedProcess([], 0, b"", b""),
            ])
            with patch.object(host, "Client", return_value=client) as constructor:
                host.watch_qemu(args)
            self.assertEqual(constructor.call_args.args[:4], ("127.0.0.1", 12345, user, "10.0.2.2"))
            record = json.loads((root / host.transfer.TOOLCHAIN_RECEIPT).read_text())
            self.assertEqual({key: record[key] for key in ("nonce", "uid", "path", "sha256", "stdoutHex", "status")},
                             dict(nonce=nonce, uid=1000, path=path, sha256=digest, stdoutHex="68656c6c6f0a", status=0))
            self.assertEqual(json.loads(client.ssh.call_args_list[2].kwargs["data"]), record)

    def test_toolchain_record_bound_to_nonce(self):
        nonce = "a" * 32
        fingerprint = "SHA256:" + "B" * 43
        peer = "192.0.2.1"
        path = f"/home/fixture/.toolchain-{nonce}/hello"
        digest = "c" * 64
        request = host.transfer.TOOLCHAIN_READY.fullmatch(
            f"SUITE toolchain HOSTSSH c-core/hello-over-ssh "
            f"nonce={nonce} fp={fingerprint} peer={peer} user=fixture uid=1000 gid=1001"
        )
        self.assertIsNotNone(request)

        client = host.Client("192.0.2.2", 22, "fixture", peer, "fixture-key", lambda: True)
        client.pin = Mock(return_value=True)
        retain = Mock()
        for reply_nonce in ("d" * 32, nonce):
            with self.subTest(reply_nonce=reply_nonce):
                client.ssh = Mock(side_effect=[
                    subprocess.CompletedProcess([], 0, (nonce + "\n").encode(), b""),
                    subprocess.CompletedProcess(
                        [], 0, f"{reply_nonce}\n1000\n{path}\n{digest}\nhello\n".encode(), b""
                    ),
                    subprocess.CompletedProcess([], 0, b"", b""),
                    subprocess.CompletedProcess([], 0, b"", b""),
                ])
                if reply_nonce != nonce:
                    with self.assertRaisesRegex(ValueError, "nonce"):
                        host.complete_toolchain(client, request, retain)
                    retain.assert_not_called()
                    self.assertEqual(client.ssh.call_count, 2)
                    continue

                host.complete_toolchain(client, request, retain)
                record = retain.call_args.args[0]
                self.assertEqual(
                    {key: record[key] for key in ("nonce", "uid", "path", "sha256", "stdoutHex", "status")},
                    dict(nonce=nonce, uid=1000, path=path, sha256=digest, stdoutHex="68656c6c6f0a", status=0),
                )
                calls = client.ssh.call_args_list
                self.assertIn("PATH=/opt/breenix-dev/25.2/bin:/bin", calls[1].args[1])
                self.assertIn(nonce, calls[1].args[1])
                self.assertEqual(json.loads(calls[2].kwargs["data"]), record)
                self.assertIn(nonce, calls[3].args[1])


if __name__ == "__main__":
    unittest.main()
