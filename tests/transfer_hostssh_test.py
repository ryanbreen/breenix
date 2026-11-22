"""Exercise the host SSH adapter without opening a network connection."""

import json
import subprocess
import unittest
from unittest.mock import Mock

from metal_tools_test import load_script


host = load_script("transfer-hostssh")


class TransferHostSsh(unittest.TestCase):
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
