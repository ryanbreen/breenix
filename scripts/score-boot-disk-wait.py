#!/usr/bin/env python3
"""Score issue 508's boot continuation evidence, including absent/duplicate markers."""
import re
import sys
from pathlib import Path

PREFIX = "[BOOT_DISK_WAIT_ORACLE:"
FINAL = "[TEST_EXEC_BLOCK_COMPLETE:x86:calls=64]"
PASS = "[BOOT_DISK_WAIT_ORACLE:x86:switched_away=0:tests_completed=1:PASS]"


def score(text):
    markers = re.findall(r"\[BOOT_DISK_WAIT_ORACLE:[^\]\r\n]*\]", text)
    return (text.count(PREFIX) == 1 and markers == [PASS]
            and text.count(FINAL) == 1
            and text.index(FINAL) < text.index(PASS))


def main():
    text = "\n".join(Path(path).read_text(errors="replace") for path in sys.argv[1:])
    if not score(text):
        print("boot disk wait oracle: FAIL (requires exactly one completed, unswitched boot window)")
        return 1
    print("boot disk wait oracle: PASS (switched_away=0 tests_completed=1)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
