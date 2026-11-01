#!/usr/bin/env python3
"""A1 acceptance must use one complete, clean default-init capture."""

import importlib.util
from pathlib import Path
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("a1_capture", ROOT / "scripts/check-arm-a1-capture.py")
SCORER = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(SCORER)

SERIAL = """[init] Breenix init starting (PID 1)
[init] test controls absent
[init] Boot script completed
[init] bsshd started (PID 22)
[init] blogd started (PID 24)
[init] ntp started (PID 25)
A1_DEFAULT_SERVICE_SNAPSHOT_BEGIN
SERVICE              STATE         PID STARTS  LAST-EXIT  USER     NOTE
bsshd                running        22      1  -          root     up 2s
blogd                running        24      1  -          root     up 2s
ntp                  running        25      1  -          root     up 2s
supervisor pid=1 began=1 entropy=1 network-up=1
service bsshd state=running user=root restart=always needs=entropy pid=22 starts=1
service blogd state=running user=root restart=always needs=- pid=24 starts=1
service ntp state=running user=root restart=on-failure needs=network-up pid=25 starts=1
  sl  local_address rem_address   st
   0: 00000000:0016 00000000:0000 0A
A1_TELNET_PORT_CLOSED
A1_DEFAULT_SERVICE_SNAPSHOT_END
SUITE users DONE passed=110 failed=0 skipped=0 total=110
"""


class A1Capture(unittest.TestCase):
    def score(self, serial=SERIAL, status="0", dirty="0"):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            for name, value in {"serial.txt": serial, "exit-status": status,
                                "commit": "a" * 40, "dirty": dirty}.items():
                (path / name).write_bytes(value if isinstance(value, bytes) else value.encode())
            return SCORER.score(path)

    def test_complete_single_capture(self):
        self.assertTrue(self.score()["proven"])

    def test_serial_bytes_do_not_discard_complete_records(self):
        result = self.score(b"\xff\n" + SERIAL.encode())
        self.assertTrue(result["proven"], result["reasons"])
        self.assertEqual(result["users"]["passed"], 110)

    def test_wrapper_success_cannot_override_boot_exit(self):
        result = self.score(status="1")
        self.assertFalse(result["proven"])
        self.assertIn("boot exit-status is 1", result["reasons"])

    def test_users_success_without_default_services_is_not_proof(self):
        result = self.score("SUITE users DONE passed=110 failed=0 skipped=0 total=110\n")
        self.assertFalse(result["proven"])
        self.assertIn("default-init service snapshot missing", result["reasons"])

    def test_default_services_cannot_hide_failed_or_missing_users(self):
        for users in ("SUITE users DONE passed=108 failed=2 skipped=0 total=110", ""):
            with self.subTest(users=users):
                self.assertFalse(self.score(SERIAL.replace(
                    "SUITE users DONE passed=110 failed=0 skipped=0 total=110", users))["proven"])

    def test_positive_port_claim_requires_tcp_table_without_listener(self):
        for table in ("", "   0: 00000000:0913 00000000:0000 0A\n",
                      "   0: 0100007F:0913 00000000:0000 0A\n"):
            with self.subTest(table=table):
                serial = SERIAL.replace("  sl  local_address rem_address   st\n"
                                        "   0: 00000000:0016 00000000:0000 0A\n", table)
                self.assertFalse(self.score(serial)["proven"])
        self.assertFalse(self.score(SERIAL + "[init] telnetd started (PID 99)\n")["proven"])

    def test_service_state_and_capture_identity_are_required(self):
        for serial in (SERIAL.replace("service ntp state=running", "service ntp state=backoff"),
                       SERIAL.replace("[init] blogd started (PID 24)\n", ""),
                       SERIAL.replace("[init] test controls absent", "[init] test controls PRESENT")):
            with self.subTest(serial=serial):
                self.assertFalse(self.score(serial)["proven"])
        self.assertFalse(self.score(dirty="1")["proven"])


if __name__ == "__main__":
    unittest.main()
