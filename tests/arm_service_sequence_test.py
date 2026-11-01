#!/usr/bin/env python3
"""Exercise the ARM service gate's real predicates without starting QEMU."""

from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
GATE = ROOT / "docker/qemu/run-aarch64-service-sequence-gate.sh"
BOOT = "[init] Boot script completed"
XHCI = "create_process_with_argv [ARM64]: ENTRY - name='xhci_counters'"
BOUNCE = "[spawn] path='/usr/bin/bounce'"
REFUSAL = "[INIT_GROUP_REFUSAL:aarch64:phase=quiesce:probe1=-22:probe2=-22:expected=-22]"
WALK = "[INIT_GROUP_WALK:aarch64:rows=11:init_tgid_rows=1:foreign_tgid_rows=0:refused=4:verdict=PASS]"
TELNET = "create_process_with_argv [ARM64]: ENTRY - name='telnetd'"
SERIAL = "\n".join([
    "[BLOCK_EINTR_ORACLE:PASS]",
    "[POLL_TCP_ORACLE:PASS]",
    "[POLL_TCP_TIMEOUT]",
    "[CTX596_ORACLE:ARMED]",
    "[spawn] path='/usr/bin/xhci_counters'",
    XHCI, BOOT, BOUNCE,
    "create_process_with_argv [ARM64]: ENTRY - name='bounce'",
    REFUSAL, WALK,
    "[SCHED_STRAND_ORACLE:aarch64:samples=1:checked=1:stranded=0:]",
    "[STRAND_INJECT_ORACLE:aarch64:stranded=0]",
    "[CENSUS_WIDEN_ORACLE:aarch64:arm_target=1:baseline_reported=0:armed_reported=1:tid=2:shape=ready_queued_nondispatching:queued_nondispatching=1:queued_nondispatch_ms=1:cpu_silence_ms=1:joined=1:retired=1:PASS]",
    "[drivers] Found 6 VirtIO MMIO devices",
    "[drivers] Found VirtIO MMIO device: network",
    "[drivers] Found VirtIO MMIO device: block",
    *("[heartbeat]" for _ in range(5)),
]) + "\n"


class ArmServiceSequence(unittest.TestCase):
    def score(self, serial):
        source = GATE.read_text()
        # Extract function definitions and the classifier's constants without
        # executing the gate's preflight, builds, resource locks or boot loop.
        functions = source[source.index("silent_spawn_reason() {"):
                           source.index("\nTOTAL_575=0")]
        constants = "\n".join(line for line in source.splitlines() if line.startswith((
            "CENSUS_WIDEN_ORACLE_PATTERN=", "EXPECTED_MMIO_DEVICES=")))
        # BASH_SOURCE must be the gate when its device census reads its flags.
        constants = constants.replace('"${BASH_SOURCE[0]}"', '"$2"')
        script = constants + "\n" + functions + '''
if green_sequence_complete "$1"; then ready=yes; else ready=no; fi
classify_serial "$1"
printf '%s\n%s\n%s\n' "$ready" "$CLASS_BUCKET" "$CLASS_REASON"
'''
        with tempfile.TemporaryDirectory(prefix="arm-service-sequence-") as directory:
            path = Path(directory) / "serial.txt"
            path.write_text(serial)
            result = subprocess.run(["bash", "-u", "-c", script, "gate-check", str(path), str(GATE)],
                                    capture_output=True, text=True, check=True)
        ready, bucket, reason = result.stdout.splitlines()
        return ready, bucket, reason

    def test_current_boot_completes_without_telnetd(self):
        self.assertEqual(self.score(SERIAL)[:2], ("yes", "GREEN"))

    def test_telnetd_start_prevents_completion_even_with_all_markers(self):
        ready, bucket, reason = self.score(SERIAL + TELNET + "\n")
        self.assertEqual(ready, "no", reason)
        self.assertNotEqual(bucket, "GREEN", reason)

    def test_required_sequence_and_five_heartbeats_are_preserved(self):
        for marker in (BOOT, XHCI, BOUNCE):
            with self.subTest(marker=marker):
                ready, bucket, reason = self.score(SERIAL.replace(marker + "\n", ""))
                self.assertEqual(ready, "no", reason)
                self.assertNotEqual(bucket, "GREEN", reason)
        self.assertEqual(self.score(SERIAL.replace("[heartbeat]\n", "", 1))[0], "no")

    def test_init_group_checks_still_run_without_telnetd(self):
        for serial in (SERIAL.replace(REFUSAL + "\n", ""),
                       SERIAL.replace(WALK + "\n", ""),
                       SERIAL.replace("rows=11:", "rows=7:"),
                       SERIAL + "[INIT_GROUP_CHILD_RAN]\n"):
            with self.subTest(serial=serial):
                ready, bucket, reason = self.score(serial)
                self.assertEqual(ready, "yes", reason)
                self.assertEqual(bucket, "P5B", reason)


if __name__ == "__main__":
    unittest.main()
