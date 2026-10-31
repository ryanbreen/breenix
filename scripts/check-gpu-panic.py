#!/usr/bin/env python3
"""Check a stopped capture-panic boot's serial record and device pixel result."""

import argparse
from pathlib import Path
import re


def fields(row):
    pairs = re.findall(r"(\w+)=([^ ]+)", row)
    if len({key for key, _ in pairs}) != len(pairs):
        return None
    return dict(pairs)


def check(text):
    captures = list(re.finditer(r"\[BXCAP:BEGIN ([^\]\r\n]+)\]", text))
    ends = list(re.finditer(r"\[BXCAP:END ([^\]\r\n]+)\]", text))
    if len(captures) != 1 or len(ends) != 1:
        return False, "expected one complete capture"
    begin, end = (fields(row) for row in (captures[0].group(1), ends[0].group(1)))
    if begin is None or end is None:
        return False, "duplicate capture field"
    if not (
        begin.get("edge") == end.get("edge") == "PANIC"
        and begin.get("seq") is not None
        and begin["seq"].isdigit()
        and begin["seq"] == end.get("seq")
        and end.get("verdict") == "complete"
        and end.get("truncated") == "0"
        and int(end.get("sections_skipped", "1"), 0) == 0
    ):
        return False, "PANIC capture is incomplete or mismatched"
    screens = list(re.finditer(r"\[GPU_PANIC_SCREEN ([^\]\r\n]+)\]", text))
    if len(screens) != 1:
        return False, "expected one device screen observation"
    screen = fields(screens[0].group(1))
    if screen is None:
        return False, "duplicate screen field"
    if any(screen.get(key) != value for key, value in {
        "shown": "1", "observed": "1", "pixels": "4096000", "mismatches": "0",
    }.items()):
        return False, "panic screen did not match the full 2560x1600 target"
    if not captures[0].start() < ends[0].start() < screens[0].start():
        return False, "panic capture and screen observation are out of order"
    if re.search(r"edge=LOCKUP|kind=LOCKUP|soft[ -]lockup", text, re.IGNORECASE):
        return False, "boot contains a soft-lockup record"
    return True, "complete PANIC; 4096000 device pixels matched; no soft-lockup record"


def check_native(text):
    ok, detail = check(text)
    if not ok:
        return ok, detail
    banner = re.search(r"KERNEL PANIC: capture_panic_oracle: deliberate panic[^\r\n]*", text)
    if banner is None or banner.start() > text.index("[BXCAP:BEGIN"):
        return False, "missing deliberate serial panic record"
    rows = re.findall(r"\[GPU_PANIC_NATIVE ([^\]\r\n]+)\]", text)
    if len(rows) != 1:
        return False, "expected one native bound-target observation"
    row = fields(rows[0])
    if (row is None or not row.get("target", "").isdigit() or int(row["target"]) == 0
            or row.get("bound") != row["target"] or row.get("poisons") != "55,aa"
            or row.get("equal") != "1" or row.get("firmware_writes") != "0"):
        return False, "native panic binding, poisons or firmware writes failed"
    return True, detail


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("serial", type=Path)
    parser.add_argument("--native", action="store_true")
    args = parser.parse_args()
    try:
        ok, detail = (check_native if args.native else check)(args.serial.read_text(errors="replace"))
    except (OSError, ValueError) as error:
        ok, detail = False, str(error)
    print(f"{'PASS' if ok else 'FAIL'}: {detail}")
    return 0 if ok else 1


if __name__ == "__main__":
    raise SystemExit(main())
