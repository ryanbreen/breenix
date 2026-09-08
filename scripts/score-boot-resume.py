#!/usr/bin/env python3
"""Score the pre-userspace register witnesses and scheduling loopback gates (567)."""
import re
import sys
from pathlib import Path

TESTS = (
    "loopback_recv_wake_when_idle",
    "loopback_recv_wake_under_load",
    "loopback_pump_does_not_busy_spin",
    "tcp_final_ack_survives_accept_publish_race",
    "loopback_wake_loss_counters_are_zero",
)


def score(text):
    witnesses = re.findall(r"\[BOOT_RESUME_ORACLE:[^\]\r\n]*\]", text)
    if witnesses != ["[BOOT_RESUME_ORACLE:x86:cycles=32:mismatch=0:PASS]"]:
        raise ValueError("expected one 32-cycle, zero-mismatch x86 resume result")
    for name in TESTS:
        events = re.findall(r"\[TEST:network:" + name + r":([^\]\r\n]*)\]", text)
        if events != ["START", "PASS"]:
            raise ValueError(f"{name}: expected one START followed by one PASS")


if __name__ == "__main__":
    try:
        score(Path(sys.argv[1]).read_text())
    except (ValueError, OSError, IndexError) as error:
        print(f"boot resume scorer: FAIL: {error}", file=sys.stderr)
        sys.exit(1)
    print("boot resume scorer: PASS (32 cycles, zero mismatches, five loopback tests)")
